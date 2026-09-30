import { UndraError } from "../errors.js";

/*
 * The standard ports of docs/SPEC.md section 8, as the host sees them: the
 * record types that cross the boundary, the typed errors, and one small
 * interface per port that a platform adapter implements. The wire codecs are
 * in `./codecs.ts`; `./ports.ts` adapts these interfaces to `PortImpl`.
 */

// ---------------------------------------------------------------------------
// Records and enums (SPEC 8)
// ---------------------------------------------------------------------------

/** `HttpMethod`, a unit enum; the wire index is the position in this list. */
export const HTTP_METHODS = ["get", "post", "put", "delete", "patch", "head", "options"] as const;
/** An HTTP method. */
export type HttpMethod = (typeof HTTP_METHODS)[number];

/** One HTTP header. Repeated names are repeated entries. */
export interface Header {
  readonly name: string;
  readonly value: string;
}

/** An HTTP request issued by the core. `timeoutMs` is `null` for "no timeout". */
export interface HttpRequest {
  readonly method: HttpMethod;
  readonly url: string;
  readonly headers: readonly Header[];
  readonly body: Uint8Array | null;
  readonly timeoutMs: number | null;
}

/** An HTTP response. Any status, including 4xx and 5xx, is a response; only transport failures are {@link HttpError}s. */
export interface HttpResponse {
  readonly status: number;
  readonly headers: readonly Header[];
  readonly body: Uint8Array;
}

/** `NetKind`, a unit enum; the wire index is the position in this list. */
export const NET_KINDS = ["wifi", "cellular", "wired", "unknown", "none"] as const;
/** The kind of network the device is on. */
export type NetKind = (typeof NET_KINDS)[number];

/** `AppState`, a unit enum; the wire index is the position in this list. */
export const APP_STATES = ["active", "inactive", "background"] as const;
/** Whether the app is in the foreground. */
export type AppState = (typeof APP_STATES)[number];

// ---------------------------------------------------------------------------
// Typed errors
// ---------------------------------------------------------------------------

/** Discriminants of {@link HttpError}. */
export type HttpErrorKind = "network" | "timeout" | "cancelled" | "invalidUrl";

/**
 * Why an HTTP request failed before a response existed. An `Http` adapter
 * rejects with one of the variants; `httpPort` sends it to the core as the
 * typed error of `Http.request`.
 */
export abstract class HttpError extends UndraError {
  declare readonly kind: HttpErrorKind;
}

/** The variants of {@link HttpError}. */
export namespace HttpError {
  /** The connection failed (DNS, refused, reset, TLS, CORS ...). */
  export class Network extends HttpError {
    declare readonly kind: "network";
    constructor(readonly value: string) {
      super("network", `network error: ${value}`);
    }
  }

  /** The request timed out. */
  export class Timeout extends HttpError {
    declare readonly kind: "timeout";
    constructor() {
      super("timeout", "the request timed out");
    }
  }

  /** The request was cancelled. */
  export class Cancelled extends HttpError {
    declare readonly kind: "cancelled";
    constructor() {
      super("cancelled", "the request was cancelled");
    }
  }

  /** The URL is not valid for this platform. */
  export class InvalidUrl extends HttpError {
    declare readonly kind: "invalidUrl";
    constructor(readonly value: string) {
      super("invalidUrl", `invalid URL: ${value}`);
    }
  }
}

/** Discriminants of {@link FsError}. */
export type FsErrorKind = "notFound" | "denied" | "io";

/** Why a file operation failed. An `Fs` adapter rejects with one of the variants. */
export abstract class FsError extends UndraError {
  declare readonly kind: FsErrorKind;
}

/** The variants of {@link FsError}. */
export namespace FsError {
  /** The path does not exist. */
  export class NotFound extends FsError {
    declare readonly kind: "notFound";
    constructor() {
      super("notFound", "not found");
    }
  }

  /** The platform refused access to the path. */
  export class Denied extends FsError {
    declare readonly kind: "denied";
    constructor() {
      super("denied", "permission denied");
    }
  }

  /** Any other I/O failure. */
  export class Io extends FsError {
    declare readonly kind: "io";
    constructor(readonly value: string) {
      super("io", `i/o error: ${value}`);
    }
  }
}

// ---------------------------------------------------------------------------
// Adapters (what a platform implements)
// ---------------------------------------------------------------------------

/** The `Http` port. Rejects with {@link HttpError}. */
export interface HttpAdapter {
  request(req: HttpRequest): Promise<HttpResponse>;
}

/** The `Kv` port, and the `SecureStore` port, which has the same shape. */
export interface KvAdapter {
  /** The value stored under `key`, or `null`. */
  get(key: string): Promise<Uint8Array | null>;
  /** Stores `value` under `key`, replacing any previous value. */
  set(key: string, value: Uint8Array): Promise<void>;
  /** Removes `key`; a missing key is not an error. */
  delete(key: string): Promise<void>;
  /** The keys that start with `prefix`, in ascending order. */
  list(prefix: string): Promise<string[]>;
}

/** The `Fs` port. Rejects with {@link FsError}. Paths are `/`-separated and relative to the adapter's root. */
export interface FsAdapter {
  read(path: string): Promise<Uint8Array>;
  write(path: string, data: Uint8Array): Promise<void>;
  delete(path: string): Promise<void>;
  list(dir: string): Promise<string[]>;
}

/**
 * The `Timer` port: fire-and-forget. `set` arranges for `fire(timerId)` to be
 * called after `delayMs`; the runtime turns that into `TimerFired`. Timers
 * are not cancellable in v1.
 */
export interface TimerAdapter {
  set(timerId: number, delayMs: number, fire: (timerId: number) => void): void;
}

/** The `Clock` port. */
export interface ClockAdapter {
  /** Milliseconds since the Unix epoch. */
  nowMs(): number;
  /** A monotonic clock in nanoseconds. */
  monotonicNs(): bigint;
}

/** The `Rng` port. */
export interface RngAdapter {
  /** Fills `out` with cryptographically secure random bytes. */
  fill(out: Uint8Array): void;
}

/** The `Log` port: where the core's log records go. `level` is 0 trace, 1 debug, 2 info, 3 warn, 4 error, 5 fatal. */
export interface LogAdapter {
  log(level: number, target: string, message: string): void;
}

/**
 * The `Connectivity` event port, host side: a source of connectivity changes.
 * `subscribe` should report the current state once, right away (from a
 * microtask is fine), then every change, and returns the unsubscribe function.
 */
export interface ConnectivityAdapter {
  subscribe(emit: (online: boolean, kind: NetKind) => void): () => void;
}

/** The `Lifecycle` event port, host side; same contract as {@link ConnectivityAdapter}. */
export interface LifecycleAdapter {
  subscribe(emit: (state: AppState) => void): () => void;
}

/** Everything a platform provides. Each entry is optional; see `LoadOptions.adapters`. */
export interface Adapters {
  http: HttpAdapter;
  kv: KvAdapter;
  secureStore: KvAdapter;
  fs: FsAdapter;
  timer: TimerAdapter;
  clock: ClockAdapter;
  rng: RngAdapter;
  log: LogAdapter;
  connectivity: ConnectivityAdapter;
  lifecycle: LifecycleAdapter;
}

/**
 * Adapters to use in place of the defaults: a value replaces the default of
 * that port, `null` removes it (the core sees the port as unavailable).
 */
export type AdapterOverrides = { readonly [K in keyof Adapters]?: Adapters[K] | null };
