import { KeelError, KeelSchemaMismatchError, KeelTransportError } from "../errors.js";
import { errorMessage, hostPlatform } from "../platform.js";
import { RUNTIME_VERSION } from "../version.js";
import {
  type HelloPayload,
  Kind,
  PortStatus,
  WireError,
  decodeEnvelope,
  decodeHello,
  decodeLog,
  decodePortCall,
  encodeEnvelope,
  encodeHello,
  encodePortReply,
} from "../wire/index.js";
import type { PortOutcome, Transport, TransportHandler } from "./transport.js";

/** The parts of the WebSocket API this transport uses (a browser `WebSocket`, Node 22's global, or a test double). */
export interface WebSocketLike {
  binaryType: string;
  readonly readyState: number;
  send(data: Uint8Array): void;
  close(code?: number, reason?: string): void;
  addEventListener(type: string, listener: (event: Event) => void): void;
  removeEventListener(type: string, listener: (event: Event) => void): void;
}

/** A constructor of {@link WebSocketLike}. */
export type WebSocketFactory = new (url: string) => WebSocketLike;

/** Options of {@link RemoteTransport}. */
export interface RemoteOptions {
  /** `ws://` or `wss://` URL of the core (`keel dev` serves one). */
  readonly url: string;
  /** The schema hash the bindings were generated from; a core reporting another one is refused. */
  readonly expectedSchemaHash: bigint;
  /** Platform name for `Hello`; default `"web"` or `"node"`. */
  readonly platform?: string;
  /** Sends `mode = "dev"` in `Hello`, which makes the core emit devtools log records (SPEC 5.10). Default `"prod"`. */
  readonly devtools?: boolean;
  /** WebSocket implementation; default the global `WebSocket`. */
  readonly webSocket?: WebSocketFactory;
  /** How long connecting and the `Hello` exchange may take, in ms. Default 10000; 0 waits forever. */
  readonly handshakeTimeoutMs?: number;
}

const OPEN = 1;
const NORMAL_CLOSURE = 1000;

/**
 * The `remote` mode: a native core (`keel dev`) reached over a WebSocket, one
 * envelope (SPEC 3.2) per binary message. The transport sends `Hello` when the
 * socket opens and refuses the core when its `Hello` carries another schema
 * hash. Port calls of the core arrive as `PortCall` envelopes and execute on
 * this side; a reply that is ready at once goes back immediately, the rest
 * through `send(Kind.PortReply, ..)`.
 *
 * There is no `callSync`, and no reconnection: a closed socket fails the
 * in-flight calls and the core has to be loaded again.
 */
export class RemoteTransport implements Transport {
  readonly mode = "remote";
  readonly synchronous = false;

  readonly #options: RemoteOptions;
  #socket: WebSocketLike | null = null;
  #handler: TransportHandler | null = null;
  #open = false;
  #closed = false;
  #seq = 0;
  #detach: (() => void) | null = null;

  /** @param options See {@link RemoteOptions}. */
  constructor(options: RemoteOptions) {
    this.#options = options;
  }

  start(handler: TransportHandler): Promise<HelloPayload> {
    this.#handler = handler;
    return new Promise<HelloPayload>((resolve, reject) => {
      const Socket = this.#options.webSocket ?? (globalThis as { WebSocket?: WebSocketFactory }).WebSocket;
      if (Socket === undefined) {
        reject(new KeelTransportError("unsupported", "this platform has no WebSocket; pass `webSocket` to use another implementation"));
        return;
      }
      let socket: WebSocketLike;
      try {
        socket = new Socket(this.#options.url);
      } catch (error) {
        reject(new KeelTransportError("handshake", `could not connect to ${this.#options.url}: ${errorMessage(error)}`, { cause: error }));
        return;
      }
      socket.binaryType = "arraybuffer";
      this.#socket = socket;
      const timeoutMs = this.#options.handshakeTimeoutMs ?? 10_000;
      let timer: ReturnType<typeof setTimeout> | undefined;
      let settled = false;
      const settle = (outcome: () => void): void => {
        if (settled) return;
        settled = true;
        if (timer !== undefined) clearTimeout(timer);
        outcome();
      };
      const abort = (error: Error): void => {
        settle(() => {
          this.close();
          reject(error);
        });
      };

      const onOpen = (): void => {
        try {
          this.#post(
            Kind.Hello,
            encodeHello({
              keelVersion: RUNTIME_VERSION,
              schemaHash: this.#options.expectedSchemaHash,
              platform: this.#options.platform ?? hostPlatform(),
              mode: this.#options.devtools === true ? "dev" : "prod",
            }),
          );
        } catch (error) {
          abort(new KeelTransportError("handshake", `could not send Hello: ${errorMessage(error)}`, { cause: error }));
        }
      };
      const onMessage = (event: Event): void => {
        const data = (event as MessageEvent).data as unknown;
        if (!(data instanceof ArrayBuffer)) {
          const error = new KeelTransportError("protocol", "the core sent a text message; Keel speaks binary envelopes");
          if (settled) this.#fail(error);
          else abort(error);
          return;
        }
        if (!settled) {
          this.#handshake(new Uint8Array(data), (hello) => {
            settle(() => {
              this.#open = true;
              resolve(hello);
            });
          }, abort);
          return;
        }
        this.#receive(new Uint8Array(data));
      };
      const onClose = (event: Event): void => {
        const { code, reason } = event as CloseEvent;
        const detail = `the connection to ${this.#options.url} closed (code ${String(code)}${reason ? `: ${reason}` : ""})`;
        if (settled) this.#fail(new KeelTransportError("closed", detail));
        else abort(new KeelTransportError("handshake", `${detail} before the core answered Hello`));
      };
      const onError = (): void => {
        // A `close` event follows and carries the details; only an error that
        // is never followed by one (a failed connect in some runtimes) needs handling here.
        if (!settled && socket.readyState !== OPEN) {
          queueMicrotask(() => {
            if (!settled) abort(new KeelTransportError("handshake", `could not connect to ${this.#options.url}`));
          });
        }
      };
      socket.addEventListener("open", onOpen);
      socket.addEventListener("message", onMessage);
      socket.addEventListener("close", onClose);
      socket.addEventListener("error", onError);
      this.#detach = () => {
        socket.removeEventListener("open", onOpen);
        socket.removeEventListener("message", onMessage);
        socket.removeEventListener("close", onClose);
        socket.removeEventListener("error", onError);
      };
      if (timeoutMs > 0) {
        timer = setTimeout(() => {
          abort(new KeelTransportError("timeout", `${this.#options.url} did not complete the Hello exchange within ${timeoutMs} ms`));
        }, timeoutMs);
      }
    });
  }

  send(kind: Kind, payload: Uint8Array): void {
    if (!this.#open || this.#socket === null) {
      throw new KeelTransportError("closed", this.#closed ? "the connection is closed" : "the core is not connected");
    }
    this.#post(kind, payload);
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#open = false;
    this.#handler = null;
    this.#detach?.();
    const socket = this.#socket;
    this.#socket = null;
    try {
      socket?.close(NORMAL_CLOSURE);
    } catch {
      // Already closing.
    }
  }

  #post(kind: Kind, payload: Uint8Array): void {
    (this.#socket as WebSocketLike).send(encodeEnvelope(kind, this.#seq++ >>> 0, this.#options.expectedSchemaHash, payload));
  }

  /** The first message must be the core's `Hello`; check its schema hash. */
  #handshake(bytes: Uint8Array, done: (hello: HelloPayload) => void, abort: (error: Error) => void): void {
    try {
      // Not judged by the header: a mismatch must be readable to be reported.
      const envelope = decodeEnvelope(bytes);
      if (envelope.kind !== Kind.Hello) {
        abort(new KeelTransportError("handshake", `expected Hello from the core, got ${Kind[envelope.kind] ?? String(envelope.kind)}`));
        return;
      }
      const hello = decodeHello(envelope.payload);
      if (hello.schemaHash !== this.#options.expectedSchemaHash) {
        abort(new KeelSchemaMismatchError(this.#options.expectedSchemaHash, hello.schemaHash));
        return;
      }
      done(hello);
    } catch (error) {
      abort(new KeelTransportError("handshake", `bad Hello from the core: ${errorMessage(error)}`, { cause: error }));
    }
  }

  /** The socket died: tell the handler once. */
  #fail(error: Error): void {
    const handler = this.#handler;
    if (this.#closed || handler === null) return;
    this.close();
    handler.closed(error);
  }

  #receive(bytes: Uint8Array): void {
    const handler = this.#handler;
    if (handler === null) return;
    try {
      const envelope = decodeEnvelope(bytes, this.#options.expectedSchemaHash);
      switch (envelope.kind) {
        case Kind.Reply:
          handler.reply(envelope.payload);
          return;
        case Kind.ChangeSet:
          handler.changeSet(envelope.payload);
          return;
        case Kind.StreamItem:
          handler.streamItem(envelope.payload);
          return;
        case Kind.Log: {
          const log = decodeLog(envelope.payload);
          handler.log(log.level, log.target, log.message);
          return;
        }
        case Kind.PortCall: {
          const call = decodePortCall(envelope.payload);
          this.#answerPortCall(call.portCallId, handler.portCall(call));
          return;
        }
        default:
          // A repeated Hello, a Snapshot, or a host-bound kind sent the wrong way: ignored.
          return;
      }
    } catch (error) {
      if (error instanceof WireError && error.detail.code === "schema_mismatch") {
        this.#fail(new KeelSchemaMismatchError(error.detail.expected, error.detail.got));
      } else if (error instanceof KeelError) {
        this.#fail(error);
      } else {
        this.#fail(new KeelTransportError("protocol", `bad message from the core: ${errorMessage(error)}`, { cause: error }));
      }
    }
  }

  #answerPortCall(portCallId: number, outcome: PortOutcome): void {
    if (outcome.kind === "sync") this.#post(Kind.PortReply, outcome.reply);
    else if (outcome.kind === "unavailable") {
      this.#post(Kind.PortReply, encodePortReply({ portCallId, status: PortStatus.Unavailable, body: new Uint8Array(0) }));
    }
  }
}
