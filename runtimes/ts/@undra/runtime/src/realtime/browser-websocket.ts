import { type Header, WsError, type WsMessage } from "../adapters/types.js";
import { errorMessage } from "../platform.js";
import { Inbox } from "./inbox.js";
import type { WebSocketAdapter, WebSocketConnection } from "./websocket.js";
import { msg } from "../messages.js";

/** What {@link browserWebSocket} needs of a WHATWG `WebSocket` (browsers, Node 22+'s global, React Native's). */
export interface PlatformWebSocket {
  /** The subprotocol the server selected, `""` for none. */
  readonly protocol: string;
  /** Bytes queued by `send` and not yet transmitted. */
  readonly bufferedAmount: number;
  /** 0 connecting, 1 open, 2 closing, 3 closed. */
  readonly readyState: number;
  /** Set to `"arraybuffer"` by the adapter. */
  binaryType: string;
  /** Queues a text or binary message. */
  send(data: string | ArrayBufferView): void;
  /** Starts the closing handshake; browsers and Node throw for a code other than 1000 or 3000-4999. */
  close(code?: number, reason?: string): void;
  /** Listens for `open`, `message`, `error` and `close`. */
  addEventListener(type: string, listener: (event: Event) => void): void;
  /** Stops listening. */
  removeEventListener(type: string, listener: (event: Event) => void): void;
}

/** Headers as {@link browserWebSocket} hands them to a constructor that takes them (repeated names joined with `", "`). */
export type WebSocketHeaderInit = Record<string, string>;

/**
 * A `WebSocket` constructor. Without headers it is called as `new WebSocket(url, protocols)`; with
 * headers (`headers: "pass"`) as Node's `new WebSocket(url, { protocols, headers })` under Node and
 * as React Native's `new WebSocket(url, protocols, { headers })` elsewhere.
 */
export type WebSocketConstructorLike = new (
  url: string,
  protocols?: string | string[] | { readonly protocols: string[]; readonly headers: WebSocketHeaderInit },
  options?: { readonly headers: WebSocketHeaderInit },
) => PlatformWebSocket;

/** Options of {@link browserWebSocket}. */
export interface BrowserWebSocketOptions {
  /** The `WebSocket` constructor; default the global one. */
  readonly WebSocket?: WebSocketConstructorLike;
  /**
   * What a connect with headers does: `"refuse"` rejects it (`Refused`, browsers cannot send
   * headers with the upgrade, and dropping them silently would leak a request without its
   * credential), `"pass"` hands them to the constructor (Node 22+, React Native). Default `"refuse"`
   * in a browser, `"pass"` under Node.
   */
  readonly headers?: "refuse" | "pass";
  /** How many received messages wait for the core before the connection is given up (1008). Default 4,096. */
  readonly maxBufferedMessages?: number;
  /**
   * How many bytes of received messages wait before the connection is given up (1008); text counts its UTF-16 length.
   * Default 16 MiB. It bounds a backlog: one message with nothing queued before it is taken whatever its size.
   */
  readonly maxBufferedBytes?: number;
}

/** The message of a connect with headers on a platform that cannot send them (ADR-047 §5). */
export const HEADERS_REFUSED = msg(114);

/** The close reason of a connection whose core stopped reading (ADR-047 §3). */
export const DID_NOT_KEEP_UP = msg(115);

const OPEN = 1;
const MAX_OUTBOUND = 1 << 20;
const POLL_MS = 16;

/** Whether this is Node (or a runtime with its `process.versions.node`). */
function isNode(): boolean {
  return typeof (globalThis as { process?: { versions?: { node?: unknown } } }).process?.versions?.node === "string";
}

/** Whether this is a browser page or worker: a `document` (or a worker scope) and not Node. */
function isBrowser(): boolean {
  const g = globalThis as { document?: unknown; WorkerGlobalScope?: unknown };
  return !isNode() && (g.document !== undefined || g.WorkerGlobalScope !== undefined);
}

/** `headers` as one record, repeated names joined with `", "`. */
function headerRecord(headers: readonly Header[]): WebSocketHeaderInit {
  const out: WebSocketHeaderInit = {};
  for (const { name, value } of headers) out[name] = Object.hasOwn(out, name) ? `${out[name]}, ${value}` : value;
  return out;
}

/** Whether a script may send `code` in a close frame (WHATWG: 1000, or 3000 to 4999). */
const scriptMaySend = (code: number): boolean => code === 1000 || (code >= 3000 && code <= 4999);

/** Closes `socket` with `code` and `reason` where the platform allows it, else without a code (browsers and Node refuse 1001 and 1008 from script). */
function closeSocket(socket: PlatformWebSocket, code: number, reason: string): void {
  try {
    socket.close(code, reason);
    return;
  } catch {
    // InvalidAccessError (a code scripts may not send) or SyntaxError (a reason over 123 bytes).
  }
  try {
    if (scriptMaySend(code)) socket.close(code);
    else socket.close();
  } catch {
    // Already closing.
  }
}

function isArrayBuffer(data: unknown): data is ArrayBuffer {
  return Object.prototype.toString.call(data) === "[object ArrayBuffer]";
}

/** One connection of {@link browserWebSocket}. */
class BrowserConnection implements WebSocketConnection {
  readonly #socket: PlatformWebSocket;
  readonly #inbox = new Inbox<WsMessage>();
  readonly #maxMessages: number;
  readonly #maxBytes: number;
  #closedByCore = false;

  constructor(socket: PlatformWebSocket, maxMessages: number, maxBytes: number) {
    this.#socket = socket;
    this.#maxMessages = maxMessages;
    this.#maxBytes = maxBytes;
  }

  get protocol(): string {
    return this.#socket.protocol ?? "";
  }

  /** A message arrived. Past the limits the connection is given up: closed (1008 where allowed) and ended with `Closed(1008)`. */
  received(data: unknown): void {
    if (this.#inbox.done) return;
    let message: WsMessage;
    let size: number;
    if (typeof data === "string") {
      message = { kind: "text", value: data };
      size = data.length;
    } else if (isArrayBuffer(data)) {
      message = { kind: "binary", value: new Uint8Array(data) };
      size = data.byteLength;
    } else if (ArrayBuffer.isView(data)) {
      message = { kind: "binary", value: new Uint8Array(data.buffer, data.byteOffset, data.byteLength).slice() };
      size = data.byteLength;
    } else {
      this.#inbox.fail(new WsError.Protocol(msg(116)));
      closeSocket(this.#socket, 1003, "");
      return;
    }
    // The limits bound a backlog: a message with nothing queued before it is taken whatever its size.
    const queued = this.#inbox.length;
    if (queued > 0 && (queued + 1 > this.#maxMessages || this.#inbox.bytes + size > this.#maxBytes)) {
      this.#inbox.fail(new WsError.Closed(1008, DID_NOT_KEEP_UP));
      closeSocket(this.#socket, 1008, DID_NOT_KEEP_UP);
      return;
    }
    this.#inbox.push(message, size);
  }

  /** The socket closed: a close frame is `Closed(code, reason)`, 1006 (no close frame) is `Network`. */
  ended(code: number, reason: string): void {
    if (code === 1006) this.#inbox.fail(new WsError.Network(msg(117)));
    else this.#inbox.fail(new WsError.Closed(code, reason));
  }

  messages(): AsyncIterable<WsMessage> {
    return this.#inbox.iterable();
  }

  async send(message: WsMessage): Promise<void> {
    const end = this.#inbox.end;
    if (end !== null) throw end;
    if (this.#closedByCore || this.#socket.readyState !== OPEN) throw new WsError.Network(msg(118));
    try {
      this.#socket.send(message.value);
    } catch (error) {
      throw new WsError.Network(errorMessage(error));
    }
    while (this.#socket.bufferedAmount > MAX_OUTBOUND) {
      await new Promise((resolve) => setTimeout(resolve, POLL_MS));
      const failed = this.#inbox.end;
      if (failed !== null) throw failed;
      if (this.#socket.readyState !== OPEN) throw new WsError.Network(msg(119));
    }
  }

  close(code: number, reason: string): Promise<void> {
    if (!this.#closedByCore) {
      this.#closedByCore = true;
      this.#inbox.close();
      closeSocket(this.#socket, code, reason);
    }
    return Promise.resolve();
  }
}

/**
 * The default WebSocket adapter of browsers (and of Node 22+ and React Native, whose `WebSocket`
 * global it also drives): the WHATWG `WebSocket` (ADR-047 §9).
 *
 * A browser `WebSocket` cannot stop reading, so the connection queues what arrives until the
 * core pulls it, up to `maxBufferedMessages` (4,096) or `maxBufferedBytes` (16 MiB); past that it
 * closes the socket and the core's stream ends, after the queued messages, with
 * `Closed { code: 1008, reason: "the core did not keep up" }` (the close frame carries 1008 where
 * the platform lets a script send it, React Native; browsers and Node only allow 1000 and
 * 3000-4999 and send a close frame without a code instead, as they do for the binding's 1001).
 * `send` resolves once `bufferedAmount` is at most 1 MiB (polled every 16 ms).
 *
 * Platform readings: a browser hides why an upgrade failed (an `error` before `open` is
 * `Refused { status: null }`), cannot send headers (refused, see `headers`), and reports a text
 * frame that is not UTF-8 like a drop (1006, `Network`). {@link nodeWebSocket} has none of these
 * limits on Node.
 */
export function browserWebSocket(options: BrowserWebSocketOptions = {}): WebSocketAdapter {
  const maxMessages = Math.max(1, options.maxBufferedMessages ?? 4096);
  const maxBytes = Math.max(1, options.maxBufferedBytes ?? 16 * 1024 * 1024);
  return {
    connect(url, protocols, headers) {
      const Socket = options.WebSocket ?? (globalThis as { WebSocket?: WebSocketConstructorLike }).WebSocket;
      if (Socket === undefined) return Promise.reject(new WsError.Refused(null, msg(120)));
      const passHeaders = (options.headers ?? (isBrowser() ? "refuse" : "pass")) === "pass";
      if (headers.length > 0 && !passHeaders) return Promise.reject(new WsError.Refused(null, HEADERS_REFUSED));
      return new Promise<WebSocketConnection>((resolve, reject) => {
        let socket: PlatformWebSocket;
        try {
          const offered = [...protocols];
          if (headers.length === 0) socket = new Socket(url, offered);
          else if (isNode()) socket = new Socket(url, { protocols: offered, headers: headerRecord(headers) });
          else socket = new Socket(url, offered, { headers: headerRecord(headers) });
        } catch (error) {
          reject(new WsError.Refused(null, errorMessage(error)));
          return;
        }
        socket.binaryType = "arraybuffer";
        const connection = new BrowserConnection(socket, maxMessages, maxBytes);
        let opened = false;
        let failure = msg(121);
        socket.addEventListener("open", () => {
          opened = true;
          resolve(connection);
        });
        socket.addEventListener("message", (event) => {
          connection.received((event as MessageEvent).data);
        });
        socket.addEventListener("error", (event) => {
          const message = (event as Partial<ErrorEvent>).message;
          if (typeof message === "string" && message.length > 0) failure = message;
        });
        socket.addEventListener("close", (event) => {
          const { code, reason } = event as CloseEvent;
          if (opened) connection.ended(code, reason ?? "");
          else reject(new WsError.Refused(null, failure));
        });
      });
    },
  };
}
