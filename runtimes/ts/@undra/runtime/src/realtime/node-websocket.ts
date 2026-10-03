import { type Header, WsError, type WsMessage } from "../adapters/types.js";
import { errorMessage } from "../platform.js";
import { nodeBuiltin } from "../node-builtin.js";
import { Inbox } from "./inbox.js";
import type { WebSocketAdapter, WebSocketConnection } from "./websocket.js";
import { msg } from "../messages.js";

/*
 * `nodeWebSocket()`: the WebSocket adapter of Node, over `node:http` / `node:https` (the upgrade)
 * and the runtime's own RFC 6455 framing, because Node's global `WebSocket` (undici) hides what the
 * port must report: the status of a refused upgrade, the close code 1001 (scripts may only send 1000
 * and 3000-4999), a text frame that is not UTF-8 (reported as a drop), and it cannot stop reading.
 * Node's modules are reached on first use through `process.getBuiltinModule`, which no bundler sees,
 * so this file costs a browser or React Native bundle nothing when it is not used and breaks nothing
 * when it is bundled.
 */

/** The parts of a Node `net.Socket` the adapter uses. */
interface NodeSocket {
  readonly destroyed: boolean;
  readonly writableLength: number;
  write(data: Uint8Array): boolean;
  end(): void;
  destroy(): void;
  pause(): void;
  resume(): void;
  setNoDelay(noDelay?: boolean): void;
  on(event: "data", listener: (chunk: Uint8Array) => void): void;
  on(event: "end" | "close" | "drain", listener: () => void): void;
  on(event: "error", listener: (error: Error) => void): void;
  once(event: "drain" | "close", listener: () => void): void;
  removeListener(event: "drain" | "close", listener: () => void): void;
}

/** The parts of an `http.IncomingMessage` the adapter reads. */
interface NodeResponse {
  readonly statusCode?: number;
  readonly statusMessage?: string;
  readonly headers: Readonly<Record<string, string | string[] | undefined>>;
  resume(): void;
}

/** The parts of an `http.ClientRequest` the adapter uses. */
interface NodeRequest {
  on(event: "upgrade", listener: (response: NodeResponse, socket: NodeSocket, head: Uint8Array) => void): void;
  on(event: "response", listener: (response: NodeResponse) => void): void;
  on(event: "error", listener: (error: Error) => void): void;
  end(): void;
  destroy(): void;
}

/** `node:http` and `node:https`, as far as the adapter needs them. */
interface NodeHttp {
  request(url: string, options: { readonly method: string; readonly headers: Record<string, string>; readonly agent: false }): NodeRequest;
}

/** Options of {@link nodeWebSocket}. */
export interface NodeWebSocketOptions {
  /** How long the closing handshake may take before the socket is destroyed, in ms. Default 1,000. */
  readonly closeTimeoutMs?: number;
  /** The largest message accepted, in bytes; a larger one closes the connection with 1009 and ends it with `Protocol`. Default 64 MiB. */
  readonly maxMessageBytes?: number;
  /** How many received messages the connection holds before it stops reading the socket (TCP then pushes back on the server). Default 16. */
  readonly readAheadMessages?: number;
  /** How many bytes of received messages it holds before it stops reading the socket. Default 1 MiB. */
  readonly readAheadBytes?: number;
}

const GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_OUTBOUND = 1 << 20;
const encoder = new TextEncoder();

function base64(bytes: Uint8Array): string {
  let text = "";
  for (const b of bytes) text += String.fromCharCode(b);
  return btoa(text);
}

/** The `Sec-WebSocket-Accept` a server must answer for `key` (RFC 6455 §4.2.2). */
async function acceptFor(key: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-1", encoder.encode(key + GUID));
  return base64(new Uint8Array(digest));
}

/** The first value of a response header. */
function headerValue(response: NodeResponse, name: string): string | undefined {
  const value = response.headers[name];
  return Array.isArray(value) ? value[0] : value;
}

/** Whether `code` may be sent in a close frame (RFC 6455 §7.4: not 1004, 1005, 1006 or 1015; 1000 to 4999). */
function sendableCode(code: number): boolean {
  return code >= 1000 && code <= 4999 && code !== 1004 && code !== 1005 && code !== 1006 && code !== 1015;
}

/** `reason` as at most 123 bytes of UTF-8 (a control frame's payload is at most 125 bytes with the code). */
function closeReason(reason: string): Uint8Array {
  const bytes = encoder.encode(reason);
  if (bytes.length <= 123) return bytes;
  let cut = 123;
  while (cut > 0 && ((bytes[cut] as number) & 0xc0) === 0x80) cut--;
  return bytes.subarray(0, cut);
}

/** A client frame: FIN, `opcode`, masked with a fresh key (RFC 6455 §5.3). */
function clientFrame(opcode: number, payload: Uint8Array): Uint8Array {
  const length = payload.length;
  const extra = length < 126 ? 0 : length < 65536 ? 2 : 8;
  const frame = new Uint8Array(2 + extra + 4 + length);
  frame[0] = 0x80 | opcode;
  if (extra === 0) {
    frame[1] = 0x80 | length;
  } else if (extra === 2) {
    frame[1] = 0x80 | 126;
    frame[2] = length >>> 8;
    frame[3] = length & 0xff;
  } else {
    frame[1] = 0x80 | 127;
    new DataView(frame.buffer).setBigUint64(2, BigInt(length));
  }
  const at = 2 + extra;
  const mask = crypto.getRandomValues(new Uint8Array(4));
  frame.set(mask, at);
  for (let i = 0; i < length; i++) frame[at + 4 + i] = (payload[i] as number) ^ (mask[i & 3] as number);
  return frame;
}

/** Received bytes, as chunks, read from the front. */
class ByteQueue {
  readonly #chunks: Uint8Array[] = [];
  #length = 0;

  get length(): number {
    return this.#length;
  }

  push(chunk: Uint8Array): void {
    if (chunk.length === 0) return;
    this.#chunks.push(chunk);
    this.#length += chunk.length;
  }

  /** The byte at `index` from the front (`index < length`). */
  at(index: number): number {
    for (const chunk of this.#chunks) {
      if (index < chunk.length) return chunk[index] as number;
      index -= chunk.length;
    }
    throw new RangeError(msg(130));
  }

  /** Removes and returns the first `n` bytes (`n <= length`). */
  take(n: number): Uint8Array {
    const out = new Uint8Array(n);
    let filled = 0;
    while (filled < n) {
      const chunk = this.#chunks[0] as Uint8Array;
      const used = Math.min(chunk.length, n - filled);
      out.set(chunk.subarray(0, used), filled);
      filled += used;
      if (used === chunk.length) this.#chunks.shift();
      else this.#chunks[0] = chunk.subarray(used);
    }
    this.#length -= n;
    return out;
  }
}

/** One connection of {@link nodeWebSocket}. */
class NodeConnection implements WebSocketConnection {
  readonly protocol: string;
  readonly #socket: NodeSocket;
  readonly #inbox = new Inbox<WsMessage>();
  readonly #bytes = new ByteQueue();
  readonly #decoder = new TextDecoder("utf-8", { fatal: true });
  readonly #closeTimeoutMs: number;
  readonly #maxMessage: number;
  readonly #readAheadMessages: number;
  readonly #readAheadBytes: number;
  #fragments: Uint8Array[] = [];
  #fragmentsLength = 0;
  #fragmentOpcode = 0;
  #closeSent = false;
  #closeReceived = false;
  #closedByCore = false;
  #failed = false;
  #paused = false;
  #closeTimer: ReturnType<typeof setTimeout> | undefined;

  constructor(socket: NodeSocket, protocol: string, head: Uint8Array, options: NodeWebSocketOptions) {
    this.protocol = protocol;
    this.#socket = socket;
    this.#closeTimeoutMs = options.closeTimeoutMs ?? 1000;
    this.#maxMessage = options.maxMessageBytes ?? 64 * 1024 * 1024;
    this.#readAheadMessages = Math.max(1, options.readAheadMessages ?? 16);
    this.#readAheadBytes = Math.max(1, options.readAheadBytes ?? 1 << 20);
    this.#inbox.onWait = () => this.#resume();
    this.#inbox.onTake = () => {
      if (this.#inbox.length < this.#readAheadMessages && this.#inbox.bytes < this.#readAheadBytes) this.#resume();
    };
    socket.setNoDelay(true);
    socket.on("data", (chunk) => {
      this.#bytes.push(chunk);
      this.#parse();
    });
    socket.on("end", () => {
      this.#dropped("the connection dropped without a close frame");
    });
    socket.on("close", () => {
      if (this.#closeTimer !== undefined) clearTimeout(this.#closeTimer);
      this.#dropped("the connection dropped without a close frame");
    });
    socket.on("error", (error) => {
      this.#dropped(errorMessage(error));
    });
    if (head.length > 0) {
      this.#bytes.push(head);
      this.#parse();
    }
  }

  messages(): AsyncIterable<WsMessage> {
    return this.#inbox.iterable();
  }

  async send(message: WsMessage): Promise<void> {
    const end = this.#inbox.end;
    if (end !== null) throw end;
    if (this.#closedByCore || this.#closeSent || this.#socket.destroyed) throw new WsError.Network("the WebSocket is not open");
    const payload = message.kind === "text" ? encoder.encode(message.value) : message.value;
    this.#socket.write(clientFrame(message.kind === "text" ? 0x1 : 0x2, payload));
    if (this.#socket.writableLength > MAX_OUTBOUND) await this.#drained();
  }

  close(code: number, reason: string): Promise<void> {
    if (this.#closedByCore) return Promise.resolve();
    this.#closedByCore = true;
    this.#inbox.close();
    this.#sendClose(code, reason);
    // Read on, so the server's close frame (and anything before it, discarded) arrives.
    this.#resume();
    return Promise.resolve();
  }

  /** Resolves when the socket's outbound buffer drained, or rejects when it closed first. */
  #drained(): Promise<void> {
    return new Promise((resolve, reject) => {
      const onDrain = (): void => {
        this.#socket.removeListener("close", onClose);
        resolve();
      };
      const onClose = (): void => {
        this.#socket.removeListener("drain", onDrain);
        reject((this.#inbox.end as WsError | null) ?? new WsError.Network("the connection closed while a message was queued"));
      };
      this.#socket.once("drain", onDrain);
      this.#socket.once("close", onClose);
    });
  }

  #resume(): void {
    if (!this.#paused) return;
    this.#paused = false;
    this.#socket.resume();
  }

  /** Stops reading the socket while the consumer holds enough; TCP then pushes back on the server. */
  #throttle(): void {
    if (this.#paused || this.#closedByCore || this.#inbox.done) return;
    if (this.#inbox.length >= this.#readAheadMessages || this.#inbox.bytes >= this.#readAheadBytes) {
      this.#paused = true;
      this.#socket.pause();
    }
  }

  #sendClose(code: number, reason: string): void {
    if (this.#closeSent || this.#socket.destroyed) return;
    this.#closeSent = true;
    let payload = new Uint8Array(0);
    if (sendableCode(code)) {
      const text = closeReason(reason);
      payload = new Uint8Array(2 + text.length);
      payload[0] = code >>> 8;
      payload[1] = code & 0xff;
      payload.set(text, 2);
    }
    this.#socket.write(clientFrame(0x8, payload));
    if (this.#closeReceived) {
      this.#socket.end();
    }
    this.#closeTimer = setTimeout(() => this.#socket.destroy(), this.#closeTimeoutMs);
  }

  /** The connection broke the protocol: close with `code` and end with `Protocol(why)`. */
  #fail(code: number, why: string): void {
    this.#failed = true;
    this.#inbox.fail(new WsError.Protocol(why));
    this.#sendClose(code, "");
    this.#socket.end();
  }

  #dropped(why: string): void {
    if (this.#closeReceived || this.#failed) return;
    this.#inbox.fail(new WsError.Network(why));
  }

  #parse(): void {
    this.#frames();
    if (!this.#failed) this.#throttle();
  }

  /** Handles every whole frame received so far. */
  #frames(): void {
    const q = this.#bytes;
    while (!this.#failed && !this.#closeReceived && q.length >= 2) {
      const b0 = q.at(0);
      const b1 = q.at(1);
      const fin = (b0 & 0x80) !== 0;
      const opcode = b0 & 0x0f;
      const masked = (b1 & 0x80) !== 0;
      const short = b1 & 0x7f;
      const extra = short === 126 ? 2 : short === 127 ? 8 : 0;
      const header = 2 + extra + (masked ? 4 : 0);
      if (q.length < header) return;
      let length = short;
      if (short === 126) {
        length = (q.at(2) << 8) | q.at(3);
      } else if (short === 127) {
        let big = 0;
        for (let i = 0; i < 8; i++) big = big * 256 + q.at(2 + i);
        length = big;
      }
      if ((b0 & 0x70) !== 0) return this.#fail(1002, "a frame has reserved bits set");
      if (masked) return this.#fail(1002, "the server masked a frame");
      if (length > this.#maxMessage) return this.#fail(1009, `a frame of ${length} bytes is larger than the limit (${this.#maxMessage})`);
      if (q.length < header + length) return;
      q.take(header);
      this.#frame(fin, opcode, q.take(length));
    }
  }

  #frame(fin: boolean, opcode: number, payload: Uint8Array): void {
    if (opcode >= 0x8) {
      if (!fin || payload.length > 125) return this.#fail(1002, "a control frame is fragmented or longer than 125 bytes");
      switch (opcode) {
        case 0x8:
          return this.#closeFrame(payload);
        case 0x9:
          if (!this.#closeSent && !this.#socket.destroyed) this.#socket.write(clientFrame(0xa, payload));
          return;
        case 0xa:
          return;
        default:
          return this.#fail(1002, `unknown control opcode ${opcode}`);
      }
    }
    if (opcode === 0x1 || opcode === 0x2) {
      if (this.#fragmentOpcode !== 0) return this.#fail(1002, "a new message started inside a fragmented one");
      this.#fragmentOpcode = opcode;
    } else if (opcode === 0x0) {
      if (this.#fragmentOpcode === 0) return this.#fail(1002, "a continuation frame without a message");
    } else {
      return this.#fail(1002, `unknown data opcode ${opcode}`);
    }
    this.#fragmentsLength += payload.length;
    if (this.#fragmentsLength > this.#maxMessage) return this.#fail(1009, `a message is larger than the limit (${this.#maxMessage} bytes)`);
    this.#fragments.push(payload);
    if (!fin) return;
    const parts = this.#fragments;
    const kind = this.#fragmentOpcode;
    const total = this.#fragmentsLength;
    this.#fragments = [];
    this.#fragmentsLength = 0;
    this.#fragmentOpcode = 0;
    let body: Uint8Array;
    if (parts.length === 1) {
      body = parts[0] as Uint8Array;
    } else {
      body = new Uint8Array(total);
      let at = 0;
      for (const part of parts) {
        body.set(part, at);
        at += part.length;
      }
    }
    if (this.#closedByCore) return;
    if (kind === 0x1) {
      let text: string;
      try {
        text = this.#decoder.decode(body);
      } catch {
        return this.#fail(1007, "a text message is not UTF-8");
      }
      this.#inbox.push({ kind: "text", value: text }, body.length);
    } else {
      this.#inbox.push({ kind: "binary", value: body }, body.length);
    }
  }

  #closeFrame(payload: Uint8Array): void {
    if (payload.length === 1) return this.#fail(1002, "a close frame with a one-byte payload");
    const code = payload.length >= 2 ? ((payload[0] as number) << 8) | (payload[1] as number) : 1005;
    let reason = "";
    if (payload.length > 2) {
      try {
        reason = this.#decoder.decode(payload.subarray(2));
      } catch {
        return this.#fail(1007, "a close reason is not UTF-8");
      }
    }
    this.#closeReceived = true;
    this.#inbox.fail(new WsError.Closed(code, reason));
    if (!this.#closeSent) this.#sendClose(code, "");
    this.#socket.end();
  }
}

/**
 * The WebSocket adapter of Node (ADR-047): `node:http` / `node:https` for the upgrade and the
 * runtime's own RFC 6455 framing. Unlike Node's global `WebSocket` ({@link browserWebSocket}) it
 * reports a refused upgrade's HTTP status, sends any close code (1001 when the core goes away),
 * reports a text frame that is not UTF-8 as `Protocol` (closing with 1007), sends headers, and
 * stops reading the socket while the core is not pulling (TCP pushes back on the server instead
 * of the connection buffering without bound). Node only (20.16+ or 22.3+): it reaches `node:http`
 * through `process.getBuiltinModule` on first use.
 *
 * Errors: a non-101 answer is `Refused { status }`; a connection that could not be made (DNS,
 * refused, TLS) is `Network`; a bad handshake (`Sec-WebSocket-Accept`, a subprotocol that was not
 * offered) is `Protocol`.
 */
export function nodeWebSocket(options: NodeWebSocketOptions = {}): WebSocketAdapter {
  return {
    async connect(url, protocols, headers) {
      const secure = url.startsWith("wss://");
      if (!secure && !url.startsWith("ws://")) throw new WsError.Refused(null, `invalid URL: ${url}`);
      let http: NodeHttp;
      try {
        http = nodeBuiltin<NodeHttp>(secure ? "node:https" : "node:http");
      } catch (error) {
        throw new WsError.Refused(null, `nodeWebSocket needs Node (${errorMessage(error)})`);
      }
      const key = base64(crypto.getRandomValues(new Uint8Array(16)));
      const expected = await acceptFor(key);
      const sent = requestHeaders(headers, key, protocols);
      return new Promise<WebSocketConnection>((resolve, reject) => {
        let request: NodeRequest;
        try {
          request = http.request(`http${url.slice(2)}`, { method: "GET", headers: sent, agent: false });
        } catch (error) {
          reject(new WsError.Refused(null, errorMessage(error)));
          return;
        }
        request.on("upgrade", (response, socket, head) => {
          const chosen = headerValue(response, "sec-websocket-protocol") ?? "";
          if (headerValue(response, "sec-websocket-accept") !== expected) {
            socket.destroy();
            reject(new WsError.Protocol("the server's Sec-WebSocket-Accept does not match the key"));
          } else if (chosen !== "" && !protocols.includes(chosen)) {
            socket.destroy();
            reject(new WsError.Protocol(`the server chose a subprotocol that was not offered: ${chosen}`));
          } else {
            resolve(new NodeConnection(socket, chosen, head, options));
          }
        });
        request.on("response", (response) => {
          response.resume();
          const status = response.statusCode ?? 0;
          reject(new WsError.Refused(status, `the server answered the upgrade with ${status} ${response.statusMessage ?? ""}`.trim()));
          request.destroy();
        });
        request.on("error", (error) => {
          reject(new WsError.Network(errorMessage(error)));
        });
        request.end();
      });
    },
  };
}

/** The upgrade request's headers: the caller's (repeated names joined), then the handshake's own, which win. */
function requestHeaders(headers: readonly Header[], key: string, protocols: readonly string[]): Record<string, string> {
  const own: Record<string, string> = {
    Connection: "Upgrade",
    Upgrade: "websocket",
    "Sec-WebSocket-Version": "13",
    "Sec-WebSocket-Key": key,
  };
  if (protocols.length > 0) own["Sec-WebSocket-Protocol"] = protocols.join(", ");
  const reserved = new Set(Object.keys(own).map((name) => name.toLowerCase()));
  const out: Record<string, string> = {};
  const spelled = new Map<string, string>();
  for (const { name, value } of headers) {
    const lower = name.toLowerCase();
    if (reserved.has(lower)) continue;
    const first = spelled.get(lower);
    if (first === undefined) {
      spelled.set(lower, name);
      out[name] = value;
    } else {
      out[first] = `${out[first]}, ${value}`;
    }
  }
  return { ...out, ...own };
}
