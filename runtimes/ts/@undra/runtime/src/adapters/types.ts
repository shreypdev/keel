import { UndraError } from "../errors.js";
import { errorMessage } from "../platform.js";

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
export type FsErrorKind = "notFound" | "denied" | "io" | "full" | "unavailable";

/**
 * Why a file operation failed. An `Fs` adapter rejects with one of the variants; `fsPort` sends it to the
 * core as the typed error of the method.
 */
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

  /** The disk or the storage quota is exhausted (`QuotaExceededError`, `ENOSPC`; ADR-049). */
  export class Full extends FsError {
    declare readonly kind: "full";
    constructor() {
      super("full", "the disk is full");
    }
  }

  /** There is no file system in this context (no origin private file system), or no adapter; `value` says which (ADR-049). */
  export class Unavailable extends FsError {
    declare readonly kind: "unavailable";
    constructor(readonly value: string) {
      super("unavailable", `the file system is unavailable: ${value}`);
    }
  }
}

/** Discriminants of {@link StorageError}. */
export type StorageErrorKind = "unavailable" | "full" | "locked" | "corrupt" | "io";

/**
 * Why a `Kv` or `SecureStore` operation failed (ADR-049). A storage adapter rejects with one of the variants;
 * `kvPort` and `secureStorePort` send it to the core as the typed error of the method (port status 1), where
 * the core's `Result<_, StorageError>` receives it. The core never panics over a storage failure.
 *
 * | Variant | Meaning | What the browser adapters map onto it |
 * |---|---|---|
 * | `Unavailable` | no backend in this context | no `indexedDB` ("needs IndexedDB"), no `crypto.subtle` ("needs a secure context"), a `SecurityError` |
 * | `Full` | the quota or the disk is exhausted | `QuotaExceededError`, `ENOSPC` |
 * | `Locked` | protected data cannot be read now | (not produced on the web) |
 * | `Corrupt` | stored bytes that cannot be read back; the key is still there | an `OperationError` on decrypt, a value not in the stored format |
 * | `Io` | anything else, with the platform's message | everything else |
 *
 * {@link StorageError.from} does that mapping for an adapter of your own.
 */
export abstract class StorageError extends UndraError {
  declare readonly kind: StorageErrorKind;

  /**
   * `error` as a {@link StorageError}: itself when it is one; `Full` for a `QuotaExceededError` or an
   * `ENOSPC` failure; `Unavailable` for a `SecurityError` (storage disabled for this origin or context);
   * `Io` with its message for anything else. For the adapter of a storage API: `catch (e) { throw StorageError.from(e); }`.
   */
  static from(error: unknown): StorageError {
    if (error instanceof StorageError) return error;
    const name = nameOf(error);
    if (name === "QuotaExceededError" || name === "ENOSPC") return new StorageError.Full();
    return name === "SecurityError" ? new StorageError.Unavailable(describe(error)) : new StorageError.Io(describe(error));
  }
}

/** The variants of {@link StorageError}; the wire index is the order below (`undra-ports`). */
export namespace StorageError {
  /** No adapter is registered, or the platform has no backend in this context; `value` says which. */
  export class Unavailable extends StorageError {
    declare readonly kind: "unavailable";
    constructor(readonly value: string) {
      super("unavailable", `storage is unavailable: ${value}`);
    }
  }

  /** The quota or the disk is exhausted. */
  export class Full extends StorageError {
    declare readonly kind: "full";
    constructor() {
      super("full", "the storage is full");
    }
  }

  /** Protected data cannot be read now (before the device's first unlock, a key that needs user authentication). */
  export class Locked extends StorageError {
    declare readonly kind: "locked";
    constructor() {
      super("locked", "the storage is locked");
    }
  }

  /** The stored bytes (or ciphertext) cannot be read back; the key is still there. */
  export class Corrupt extends StorageError {
    declare readonly kind: "corrupt";
    constructor(readonly value: string) {
      super("corrupt", `stored data is corrupt: ${value}`);
    }
  }

  /** Any other failure; `value` is the platform's text. */
  export class Io extends StorageError {
    declare readonly kind: "io";
    constructor(readonly value: string) {
      super("io", `storage I/O error: ${value}`);
    }
  }
}

/** The `name` of a `DOMException` (or of any error object), or the `code` of a Node.js system error (`ENOSPC`). */
function nameOf(error: unknown): unknown {
  const e = error as { name?: unknown; code?: unknown } | null;
  return typeof e?.code === "string" ? e.code : e?.name;
}

/** The text of a failure, for the `Io` and `Unavailable` variants: its message, else its name. Never throws. */
function describe(error: unknown): string {
  return errorMessage(error) || String(nameOf(error) ?? "unknown failure");
}

/**
 * `error` as an {@link FsError}: itself when it is one; `Full` for a `QuotaExceededError` or `ENOSPC`;
 * `NotFound` for a `NotFoundError` or `ENOENT`; `Denied` for a `NotAllowedError`, a `SecurityError`, `EACCES`
 * or `EPERM`; `Io` with its message for anything else. For the adapter of a file API:
 * `catch (e) { throw fsErrorFrom(e); }`.
 */
export function fsErrorFrom(error: unknown): FsError {
  if (error instanceof FsError) return error;
  switch (nameOf(error)) {
    case "QuotaExceededError":
    case "ENOSPC":
      return new FsError.Full();
    case "NotFoundError":
    case "ENOENT":
      return new FsError.NotFound();
    case "NotAllowedError":
    case "SecurityError":
    case "EACCES":
    case "EPERM":
      return new FsError.Denied();
    default:
      return new FsError.Io(describe(error));
  }
}

// ---------------------------------------------------------------------------
// Adapters (what a platform implements)
// ---------------------------------------------------------------------------

/** The `Http` port. Rejects with {@link HttpError}. */
export interface HttpAdapter {
  request(req: HttpRequest): Promise<HttpResponse>;
}

/**
 * The `Kv` port, and the `SecureStore` port, which has the same shape. Every method rejects with a
 * {@link StorageError} when the storage fails (ADR-049); {@link StorageError.from} maps what a storage API throws
 * (`QuotaExceededError`, `ENOSPC`, `SecurityError`, ...) onto it. Any other rejection is a bug in the adapter:
 * it is reported (an error-level log naming the port, and `onError`) and the core sees the port as
 * unavailable for that call.
 */
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

/**
 * The `Fs` port. Rejects with {@link FsError} ({@link fsErrorFrom} maps what a file API throws onto it; any
 * other rejection is reported and answered as unavailable). Paths are `/`-separated and relative to the adapter's root.
 */
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
