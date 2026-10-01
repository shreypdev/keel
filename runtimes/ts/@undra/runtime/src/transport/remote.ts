import { UndraError, UndraSchemaMismatchError, UndraSessionLostError, UndraTransportError } from "../errors.js";
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

/**
 * How a {@link RemoteTransport} reconnects (ADR-051). Attempt `n` (from 1) waits
 * `min(maxDelayMs, initialDelayMs * 2^(n-1))`, less a random share of up to `jitter` of that, so
 * many clients of one server do not retry in step.
 */
export interface ReconnectOptions {
  /** The wait before the first retry, in ms (at least 1). Default 250. */
  readonly initialDelayMs?: number;
  /** The longest wait, in ms. Default 5000. */
  readonly maxDelayMs?: number;
  /** The share of the wait that is randomised away, from 0 (none) to 1 (down to nothing). Default 0.5. */
  readonly jitter?: number;
  /** Give up (and close the core) after this many failed attempts. Default: never. */
  readonly maxAttempts?: number;
  /** Random numbers in `[0, 1)` for the jitter; default `Math.random`. */
  readonly random?: () => number;
}

/** Options of {@link RemoteTransport}. */
export interface RemoteOptions {
  /** `ws://` or `wss://` URL of the core (`undra dev` serves one). */
  readonly url: string;
  /** The schema hash the bindings were generated from; a core reporting another one is refused. */
  readonly expectedSchemaHash: bigint;
  /** Platform name for `Hello`; default `"web"` or `"node"`. */
  readonly platform?: string;
  /** Sends `mode = "dev"` in `Hello`, which makes the core emit devtools log records (SPEC 5.10). Default `"prod"`. */
  readonly devtools?: boolean;
  /** WebSocket implementation; default the global `WebSocket`. */
  readonly webSocket?: WebSocketFactory;
  /** How long connecting and the `Hello` exchange may take, in ms. Default 10000; 0 waits forever. A retry waits at most 5000. */
  readonly handshakeTimeoutMs?: number;
  /** Reconnect by itself after a lost connection: `false` turns it off, an object tunes it. Default on (see {@link ReconnectOptions}). */
  readonly reconnect?: boolean | ReconnectOptions;
  /**
   * The session token sent in the URL (`?undra_session=`), the same on every connection of this
   * transport, so the server can keep the objects of a dropped client for it. Default: a random one;
   * `false` sends none (the server then releases the objects when the connection drops).
   */
  readonly session?: string | false;
}

/** The longest a single reconnect attempt (connect and `Hello`) may take, whatever `handshakeTimeoutMs` says. */
const RECONNECT_ATTEMPT_CAP_MS = 5_000;

/** The close code with which the server says it no longer holds this client's session (ADR-051). */
const SESSION_LOST = 4001;

interface ResolvedPolicy {
  readonly initialDelayMs: number;
  readonly maxDelayMs: number;
  readonly jitter: number;
  readonly maxAttempts: number;
  readonly random: () => number;
}

function resolvePolicy(option: boolean | ReconnectOptions | undefined): ResolvedPolicy | null {
  if (option === false) return null;
  const given = typeof option === "object" ? option : {};
  // A wait of at least 1 ms, as Kotlin requires: zero would retry a server that is down in a hot loop.
  const initialDelayMs = Math.max(1, given.initialDelayMs ?? 250);
  return {
    initialDelayMs,
    maxDelayMs: Math.max(initialDelayMs, given.maxDelayMs ?? 5_000),
    jitter: Math.min(1, Math.max(0, given.jitter ?? 0.5)),
    maxAttempts: given.maxAttempts ?? Number.POSITIVE_INFINITY,
    random: given.random ?? Math.random,
  };
}

/**
 * How long reconnect attempt `attempt` (from 1) waits, in ms: exponential from `initialDelayMs`,
 * capped at `maxDelayMs`, with `jitter` of it randomised away. The same schedule in every runtime.
 */
export function reconnectDelayMs(attempt: number, options: ReconnectOptions = {}): number {
  const policy = resolvePolicy(options) as ResolvedPolicy;
  // At most 30 doublings (as in Kotlin and Swift): a long outage must not overflow to Infinity or NaN.
  const base = Math.min(policy.maxDelayMs, policy.initialDelayMs * 2 ** Math.min(30, Math.max(0, attempt - 1)));
  return Math.round(base * (1 - policy.jitter * policy.random()));
}

/** A random token for the session; `crypto.getRandomValues` works on insecure pages (a phone on the LAN), `randomUUID` does not. */
function newSessionToken(): string {
  const bytes = new Uint8Array(16);
  const crypto = (globalThis as { crypto?: { getRandomValues?: (a: Uint8Array) => Uint8Array } }).crypto;
  if (crypto?.getRandomValues !== undefined) crypto.getRandomValues(bytes);
  else for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/** Whether another attempt can fix `error`: a connection that dropped or was never made, not a core that is another core. */
function retryable(error: Error): boolean {
  if (error instanceof UndraTransportError) return error.reason === "closed" || error.reason === "handshake" || error.reason === "timeout";
  return false;
}

const OPEN = 1;
const NORMAL_CLOSURE = 1000;

/**
 * The `remote` mode: a native core (`undra dev`) reached over a WebSocket, one
 * envelope (SPEC 3.2) per binary message. The transport sends `Hello` when the
 * socket opens and refuses the core when its `Hello` carries another schema
 * hash. Port calls of the core arrive as `PortCall` envelopes and execute on
 * this side; a reply that is ready at once goes back immediately, the rest
 * through `send(Kind.PortReply, ..)`.
 *
 * There is no `callSync`. A connection that drops is **reconnected** (ADR-051): the
 * handler hears `reconnecting` (and what was in flight fails), the transport retries with
 * backoff and jitter, and `reconnected` follows the next successful handshake. A closed core
 * (`close()`), a core with another schema hash and a session the server lost are final: they
 * end in `closed`, never in another attempt. Every connection carries the same session token
 * in its URL, so `undra dev` can keep the objects of a client that dropped.
 */
export class RemoteTransport implements Transport {
  readonly mode = "remote";
  readonly synchronous = false;

  readonly #options: RemoteOptions;
  readonly #policy: ResolvedPolicy | null;
  readonly #session: string | null;
  #socket: WebSocketLike | null = null;
  #handler: TransportHandler | null = null;
  #open = false;
  #closed = false;
  #seq = 0;
  #detach: (() => void) | null = null;
  #timer: ReturnType<typeof setTimeout> | undefined;

  /** @param options See {@link RemoteOptions}. */
  constructor(options: RemoteOptions) {
    this.#options = options;
    this.#policy = resolvePolicy(options.reconnect);
    this.#session = options.session === false ? null : (options.session ?? newSessionToken());
  }

  start(handler: TransportHandler): Promise<HelloPayload> {
    this.#handler = handler;
    return this.#attempt(false, this.#options.handshakeTimeoutMs ?? 10_000).catch((error: unknown) => {
      this.close();
      throw error;
    });
  }

  /** The URL of one connection: the core's, with the session token (and, when objects are to be found again, the resume flag). */
  #urlFor(resume: boolean): string {
    if (this.#session === null) return this.#options.url;
    try {
      const url = new URL(this.#options.url);
      url.searchParams.set("undra_session", this.#session);
      if (resume) url.searchParams.set("undra_resume", "1");
      return url.toString();
    } catch {
      return this.#options.url;
    }
  }

  /**
   * One connection: opens a socket, performs the `Hello` exchange and resolves with the core's `Hello`.
   * A failure rejects (and drops the socket) without closing the transport: whether to retry is
   * for the caller. Once it has resolved, a lost socket is reported through {@link RemoteTransport.#fail}.
   */
  #attempt(resume: boolean, timeoutMs: number): Promise<HelloPayload> {
    return new Promise<HelloPayload>((resolve, reject) => {
      const Socket = this.#options.webSocket ?? (globalThis as { WebSocket?: WebSocketFactory }).WebSocket;
      if (Socket === undefined) {
        reject(new UndraTransportError("unsupported", "this platform has no WebSocket; pass `webSocket` to use another implementation"));
        return;
      }
      const url = this.#options.url;
      let socket: WebSocketLike;
      try {
        socket = new Socket(this.#urlFor(resume));
      } catch (error) {
        reject(new UndraTransportError("handshake", `could not connect to ${url}: ${errorMessage(error)}`, { cause: error }));
        return;
      }
      socket.binaryType = "arraybuffer";
      this.#socket = socket;
      this.#seq = 0;
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
          if (this.#socket === socket) this.#dropSocket();
          reject(error);
        });
      };

      const onOpen = (): void => {
        try {
          this.#post(
            Kind.Hello,
            encodeHello({
              undraVersion: RUNTIME_VERSION,
              schemaHash: this.#options.expectedSchemaHash,
              platform: this.#options.platform ?? hostPlatform(),
              mode: this.#options.devtools === true ? "dev" : "prod",
            }),
          );
        } catch (error) {
          abort(new UndraTransportError("handshake", `could not send Hello: ${errorMessage(error)}`, { cause: error }));
        }
      };
      const onMessage = (event: Event): void => {
        const data = (event as MessageEvent).data as unknown;
        if (!(data instanceof ArrayBuffer)) {
          const error = new UndraTransportError("protocol", "the core sent a text message; Undra speaks binary envelopes");
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
        const detail = `the connection to ${url} closed (code ${String(code)}${reason ? `: ${reason}` : ""})`;
        if (code === SESSION_LOST) {
          const lost = new UndraSessionLostError(reason || undefined);
          if (settled) this.#fail(lost);
          else abort(lost);
          return;
        }
        if (settled) this.#fail(new UndraTransportError("closed", detail));
        else abort(new UndraTransportError("handshake", `${detail} before the core answered Hello`));
      };
      const onError = (): void => {
        // A `close` event follows and carries the details; only an error that
        // is never followed by one (a failed connect in some runtimes) needs handling here.
        if (!settled && socket.readyState !== OPEN) {
          queueMicrotask(() => {
            if (!settled) abort(new UndraTransportError("handshake", `could not connect to ${url}`));
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
          abort(new UndraTransportError("timeout", `${url} did not complete the Hello exchange within ${timeoutMs} ms`));
        }, timeoutMs);
      }
    });
  }

  send(kind: Kind, payload: Uint8Array): void {
    if (!this.#open || this.#socket === null) {
      throw new UndraTransportError(
        "closed",
        this.#closed ? "the connection is closed" : this.#policy === null ? "the core is not connected" : "the core is not connected: reconnecting",
      );
    }
    this.#post(kind, payload);
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#handler = null;
    if (this.#timer !== undefined) clearTimeout(this.#timer);
    this.#timer = undefined;
    this.#dropSocket();
  }

  /** Detaches from the socket and closes it; the transport itself stays usable for another connection. */
  #dropSocket(): void {
    this.#open = false;
    this.#detach?.();
    this.#detach = null;
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
        abort(new UndraTransportError("handshake", `expected Hello from the core, got ${Kind[envelope.kind] ?? String(envelope.kind)}`));
        return;
      }
      const hello = decodeHello(envelope.payload);
      if (hello.schemaHash !== this.#options.expectedSchemaHash) {
        abort(new UndraSchemaMismatchError(this.#options.expectedSchemaHash, hello.schemaHash));
        return;
      }
      done(hello);
    } catch (error) {
      abort(new UndraTransportError("handshake", `bad Hello from the core: ${errorMessage(error)}`, { cause: error }));
    }
  }

  /** The connection died after the handshake: reconnect when that can help, else tell the handler, once. */
  #fail(error: Error): void {
    const handler = this.#handler;
    if (this.#closed || handler === null) return;
    this.#dropSocket();
    if (this.#policy !== null && retryable(error)) {
      this.#retry(1, error);
      return;
    }
    this.close();
    handler.closed(error);
  }

  /** Announces reconnect attempt `attempt` (the first is the loss itself) and schedules it after its backoff. */
  #retry(attempt: number, error: Error): void {
    const handler = this.#handler;
    const policy = this.#policy;
    if (this.#closed || handler === null || policy === null) return;
    if (attempt > policy.maxAttempts) {
      this.close();
      handler.closed(error);
      return;
    }
    handler.reconnecting?.(attempt, error);
    if (this.#closed) return; // the handler closed the core
    const wait = reconnectDelayMs(attempt, policy);
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      void this.#reconnect(attempt);
    }, wait);
  }

  async #reconnect(attempt: number): Promise<void> {
    const handler = this.#handler;
    if (this.#closed || handler === null) return;
    const patience = this.#options.handshakeTimeoutMs ?? 10_000;
    let hello: HelloPayload;
    try {
      hello = await this.#attempt(handler.holdsObjects?.() === true, patience > 0 ? Math.min(patience, RECONNECT_ATTEMPT_CAP_MS) : RECONNECT_ATTEMPT_CAP_MS);
    } catch (error) {
      if (this.#closed) return;
      const failure = error instanceof Error ? error : new UndraTransportError("handshake", errorMessage(error));
      if (retryable(failure)) {
        this.#retry(attempt + 1, failure);
      } else {
        this.close();
        handler.closed(failure);
      }
      return;
    }
    if (this.#closed) return;
    handler.reconnected?.(hello);
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
        this.#fail(new UndraSchemaMismatchError(error.detail.expected, error.detail.got));
      } else if (error instanceof UndraError) {
        this.#fail(error);
      } else {
        this.#fail(new UndraTransportError("protocol", `bad message from the core: ${errorMessage(error)}`, { cause: error }));
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
