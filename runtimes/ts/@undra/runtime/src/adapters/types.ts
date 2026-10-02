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

// `NetKind` and `AppState` are defined with the host events that use them (`./events.ts`): this module is only
// loaded with the default ports, while a page starts the event sources with its core (ADR-052).
import type { AppState, NetKind } from "./events.js";
export { APP_STATES, type AppState, NET_KINDS, type NetKind } from "./events.js";

// ---------------------------------------------------------------------------
// The opt-in ports' records and enums (ADR-047 WebSocket and Sse, ADR-048 Db)
// ---------------------------------------------------------------------------

/** What `WebSocket.connect` answers: the connection's id and the subprotocol the server chose. */
export interface WsOpened {
  /** The connection's id, chosen by the adapter; never reused by it. */
  readonly conn: number;
  /** The subprotocol the server selected, or `""` when none was negotiated. */
  readonly protocol: string;
}

/** One WebSocket message. Control frames (ping, pong, close) never surface as messages. */
export type WsMessage =
  /** A text message (valid UTF-8 by RFC 6455). */
  | { readonly kind: "text"; readonly value: string }
  /** A binary message. */
  | { readonly kind: "binary"; readonly value: Uint8Array };

/** One server-sent event. */
export interface SseEvent {
  /**
   * The event's `id` field, if the event (or an earlier one, per the standard's last-event-id
   * buffer) set one. Send it back as `lastEventId` to resume.
   */
  readonly id: string | null;
  /** The event type: the `event` field, `"message"` when absent. */
  readonly event: string;
  /** The `data` lines, joined with `\n`. */
  readonly data: string;
  /** The `retry` field in milliseconds, when this event carried one: how long the server asks clients to wait before reconnecting. */
  readonly retryMs: number | null;
}

/** One schema migration, as the `Db` port carries it. */
export interface DbMigration {
  /** The version this migration brings the database to; versions strictly increase from 1. */
  readonly version: number;
  /** The SQL, possibly several statements. */
  readonly sql: string;
}

/** What `Db.open` answers: the database's id and its version after migrating. */
export interface DbOpened {
  /** The database's id, chosen by the adapter; never reused by it. */
  readonly db: number;
  /** `PRAGMA user_version` after the migrations ran (0 for a database without any). */
  readonly version: number;
}

/** One SQLite value: the five storage classes. An `INTEGER` is a `bigint`, so every 64-bit value crosses exactly. */
export type DbValue =
  /** `NULL`. */
  | { readonly kind: "null" }
  /** A 64-bit signed integer. */
  | { readonly kind: "integer"; readonly value: bigint }
  /** A 64-bit float. */
  | { readonly kind: "real"; readonly value: number }
  /** UTF-8 text. */
  | { readonly kind: "text"; readonly value: string }
  /** Bytes. */
  | { readonly kind: "blob"; readonly value: Uint8Array };

/** What `Db.execute` answers. */
export interface DbExecuted {
  /** Rows inserted, updated or deleted by the statement (`sqlite3_changes64`). */
  readonly changes: bigint;
  /** The rowid of the last insert on the connection (`sqlite3_last_insert_rowid`). */
  readonly lastInsertId: bigint;
}

/** What `Db.query` answers: the column names and the rows, each a cell per column. */
export interface DbRows {
  /** The result's column names, in order. */
  readonly columns: string[];
  /** The rows, each with one value per column. */
  readonly rows: DbValue[][];
}

/** `DbConstraint`, a unit enum; the wire index is the position in this list. */
export const DB_CONSTRAINTS = ["unique", "notNull", "foreignKey", "check", "other"] as const;
/**
 * Which constraint a statement broke: `unique` (`UNIQUE` or `PRIMARY KEY`), `notNull`, `foreignKey`
 * (enforced: every database opens with `foreign_keys = ON`), `check`, or `other` (a trigger's `RAISE`, ...).
 */
export type DbConstraint = (typeof DB_CONSTRAINTS)[number];

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
  const name = nameOf(error);
  return name === "QuotaExceededError" || name === "ENOSPC"
    ? new FsError.Full()
    : name === "NotFoundError" || name === "ENOENT"
      ? new FsError.NotFound()
      : name === "NotAllowedError" || name === "SecurityError" || name === "EACCES" || name === "EPERM"
        ? new FsError.Denied()
        : new FsError.Io(describe(error));
}

/** Discriminants of {@link WsError}. */
export type WsErrorKind = "refused" | "network" | "protocol" | "closed";

/**
 * Why a WebSocket could not be opened, or how it ended (ADR-047 §5). A `WebSocketAdapter` rejects
 * with one of the variants (`WsError.Refused`, ...); `webSocketPort` (`@undra/runtime/realtime`)
 * sends it to the core.
 *
 * | What happened | Variant |
 * |---|---|
 * | the upgrade was answered non-101, the URL is unusable, headers the platform cannot send | `Refused` |
 * | the connection dropped without a close frame (DNS, reset, TLS, timeout) | `Network` |
 * | the peer broke RFC 6455, or a reply did not decode | `Protocol` |
 * | the peer sent a close frame (1000 included), or the adapter closed past its backlog limit (1008) | `Closed` |
 */
export abstract class WsError extends UndraError {
  declare readonly kind: WsErrorKind;

  /** The connection was not established. `status` is the HTTP status of the refused upgrade where the platform reports it (browsers do not). */
  static get Refused(): typeof WsErrorRefused {
    return WsErrorRefused;
  }

  /** The connection failed or dropped without a closing handshake; the text is the platform's. */
  static get Network(): typeof WsErrorNetwork {
    return WsErrorNetwork;
  }

  /** The peer broke the protocol (or a port reply did not decode); the text says how. */
  static get Protocol(): typeof WsErrorProtocol {
    return WsErrorProtocol;
  }

  /** The connection was closed with a close frame: `code` and `reason` are the frame's. */
  static get Closed(): typeof WsErrorClosed {
    return WsErrorClosed;
  }
}

/** The variants of {@link WsError}, as types (`WsError.Refused`). */
export declare namespace WsError {
  /** The connection was not established. `status` is the HTTP status of the refused upgrade where the platform reports it (browsers do not). */
  type Refused = WsErrorRefused;
  /** The connection failed or dropped without a closing handshake; the text is the platform's. */
  type Network = WsErrorNetwork;
  /** The peer broke the protocol (or a port reply did not decode); the text says how. */
  type Protocol = WsErrorProtocol;
  /** The connection was closed with a close frame: `code` and `reason` are the frame's. */
  type Closed = WsErrorClosed;
}

/** `WsError.Refused`: the connection was not established. `status` is the HTTP status of the refused upgrade where the platform reports it (browsers do not). Construct and test it as `WsError.Refused`. */
export class WsErrorRefused extends WsError {
  declare readonly kind: "refused";
  constructor(
    readonly status: number | null,
    readonly message_: string,
  ) {
    super("refused", `the WebSocket was refused: ${message_}`);
  }
}

/** `WsError.Network`: the connection failed or dropped without a closing handshake; the text is the platform's. Construct and test it as `WsError.Network`. */
export class WsErrorNetwork extends WsError {
  declare readonly kind: "network";
  constructor(readonly value: string) {
    super("network", `WebSocket network error: ${value}`);
  }
}

/** `WsError.Protocol`: the peer broke the protocol (or a port reply did not decode); the text says how. Construct and test it as `WsError.Protocol`. */
export class WsErrorProtocol extends WsError {
  declare readonly kind: "protocol";
  constructor(readonly value: string) {
    super("protocol", `WebSocket protocol error: ${value}`);
  }
}

/** `WsError.Closed`: the connection was closed with a close frame: `code` and `reason` are the frame's. Construct and test it as `WsError.Closed`. */
export class WsErrorClosed extends WsError {
  declare readonly kind: "closed";
  constructor(
    readonly code: number,
    readonly reason: string,
  ) {
    super("closed", `the WebSocket was closed (${code}): ${reason}`);
  }
}

/** Discriminants of {@link SseError}. */
export type SseErrorKind = "refused" | "network" | "protocol" | "ended";

/**
 * Why a server-sent event stream could not be opened, or how it ended (ADR-047 §5). An
 * `SseAdapter` rejects with one of the variants (`SseError.Refused`, ...).
 */
export abstract class SseError extends UndraError {
  declare readonly kind: SseErrorKind;

  /** The request was answered with a status other than 2xx (a 204 means "stop"), or the URL is unusable. `status` is `null` when there was no HTTP answer. */
  static get Refused(): typeof SseErrorRefused {
    return SseErrorRefused;
  }

  /** The connection failed or dropped; the text is the platform's. */
  static get Network(): typeof SseErrorNetwork {
    return SseErrorNetwork;
  }

  /** The answer was not `text/event-stream`, was not UTF-8, or a port reply did not decode. */
  static get Protocol(): typeof SseErrorProtocol {
    return SseErrorProtocol;
  }

  /** The server ended the response. Reconnect with the last event id to resume. */
  static get Ended(): typeof SseErrorEnded {
    return SseErrorEnded;
  }
}

/** The variants of {@link SseError}, as types (`SseError.Refused`). */
export declare namespace SseError {
  /** The request was answered with a status other than 2xx (a 204 means "stop"), or the URL is unusable. `status` is `null` when there was no HTTP answer. */
  type Refused = SseErrorRefused;
  /** The connection failed or dropped; the text is the platform's. */
  type Network = SseErrorNetwork;
  /** The answer was not `text/event-stream`, was not UTF-8, or a port reply did not decode. */
  type Protocol = SseErrorProtocol;
  /** The server ended the response. Reconnect with the last event id to resume. */
  type Ended = SseErrorEnded;
}

/** `SseError.Refused`: the request was answered with a status other than 2xx (a 204 means "stop"), or the URL is unusable. `status` is `null` when there was no HTTP answer. Construct and test it as `SseError.Refused`. */
export class SseErrorRefused extends SseError {
  declare readonly kind: "refused";
  constructor(
    readonly status: number | null,
    readonly message_: string,
  ) {
    super("refused", `the event stream was refused: ${message_}`);
  }
}

/** `SseError.Network`: the connection failed or dropped; the text is the platform's. Construct and test it as `SseError.Network`. */
export class SseErrorNetwork extends SseError {
  declare readonly kind: "network";
  constructor(readonly value: string) {
    super("network", `event stream network error: ${value}`);
  }
}

/** `SseError.Protocol`: the answer was not `text/event-stream`, was not UTF-8, or a port reply did not decode. Construct and test it as `SseError.Protocol`. */
export class SseErrorProtocol extends SseError {
  declare readonly kind: "protocol";
  constructor(readonly value: string) {
    super("protocol", `event stream protocol error: ${value}`);
  }
}

/** `SseError.Ended`: the server ended the response. Reconnect with the last event id to resume. Construct and test it as `SseError.Ended`. */
export class SseErrorEnded extends SseError {
  declare readonly kind: "ended";
  constructor() {
    super("ended", "the server ended the event stream");
  }
}

/** Discriminants of {@link DbError}. */
export type DbErrorKind = "busy" | "constraint" | "corrupt" | "full" | "unavailable" | "sql" | "migration";

/**
 * Why a database operation failed (ADR-048). The variant follows SQLite's result code, never the
 * message text. A `DbAdapter` rejects with one of the variants (`DbError.Busy`, ...); `dbPort`
 * (`@undra/runtime/db`) sends it to the core.
 */
export abstract class DbError extends UndraError {
  declare readonly kind: DbErrorKind;

  /** The database stayed locked past the busy timeout (5 s): another connection, or a statement made outside a running transaction on the same database. */
  static get Busy(): typeof DbErrorBusy {
    return DbErrorBusy;
  }

  /** The statement broke a constraint of kind `kind_`; `message_` is SQLite's (it names the columns). */
  static get Constraint(): typeof DbErrorConstraint {
    return DbErrorConstraint;
  }

  /** The file is not a database or is damaged. */
  static get Corrupt(): typeof DbErrorCorrupt {
    return DbErrorCorrupt;
  }

  /** The disk or the storage quota is full. */
  static get Full(): typeof DbErrorFull {
    return DbErrorFull;
  }

  /** No adapter, a database or transaction that is closed or unknown, an invalid name, a file that cannot be opened. */
  static get Unavailable(): typeof DbErrorUnavailable {
    return DbErrorUnavailable;
  }

  /** Anything else SQLite refused: a syntax error, a missing table, more than one statement where one is expected. */
  static get Sql(): typeof DbErrorSql {
    return DbErrorSql;
  }

  /** A migration failed (everything the open migrated is rolled back), or the database is newer than the newest migration. */
  static get Migration(): typeof DbErrorMigration {
    return DbErrorMigration;
  }
}

/** The variants of {@link DbError}, as types (`DbError.Busy`). */
export declare namespace DbError {
  /** The database stayed locked past the busy timeout (5 s): another connection, or a statement made outside a running transaction on the same database. */
  type Busy = DbErrorBusy;
  /** The statement broke a constraint of kind `kind_`; `message_` is SQLite's (it names the columns). */
  type Constraint = DbErrorConstraint;
  /** The file is not a database or is damaged. */
  type Corrupt = DbErrorCorrupt;
  /** The disk or the storage quota is full. */
  type Full = DbErrorFull;
  /** No adapter, a database or transaction that is closed or unknown, an invalid name, a file that cannot be opened. */
  type Unavailable = DbErrorUnavailable;
  /** Anything else SQLite refused: a syntax error, a missing table, more than one statement where one is expected. */
  type Sql = DbErrorSql;
  /** A migration failed (everything the open migrated is rolled back), or the database is newer than the newest migration. */
  type Migration = DbErrorMigration;
}

/** `DbError.Busy`: the database stayed locked past the busy timeout (5 s): another connection, or a statement made outside a running transaction on the same database. Construct and test it as `DbError.Busy`. */
export class DbErrorBusy extends DbError {
  declare readonly kind: "busy";
  constructor() {
    super("busy", "the database is busy");
  }
}

/** `DbError.Constraint`: the statement broke a constraint of kind `kind_`; `message_` is SQLite's (it names the columns). Construct and test it as `DbError.Constraint`. */
export class DbErrorConstraint extends DbError {
  declare readonly kind: "constraint";
  constructor(
    readonly kind_: DbConstraint,
    readonly message_: string,
  ) {
    super("constraint", `constraint failed: ${message_}`);
  }
}

/** `DbError.Corrupt`: the file is not a database or is damaged. Construct and test it as `DbError.Corrupt`. */
export class DbErrorCorrupt extends DbError {
  declare readonly kind: "corrupt";
  constructor(readonly value: string) {
    super("corrupt", `the database is corrupt: ${value}`);
  }
}

/** `DbError.Full`: the disk or the storage quota is full. Construct and test it as `DbError.Full`. */
export class DbErrorFull extends DbError {
  declare readonly kind: "full";
  constructor() {
    super("full", "the database is full");
  }
}

/** `DbError.Unavailable`: no adapter, a database or transaction that is closed or unknown, an invalid name, a file that cannot be opened. Construct and test it as `DbError.Unavailable`. */
export class DbErrorUnavailable extends DbError {
  declare readonly kind: "unavailable";
  constructor(readonly value: string) {
    super("unavailable", `the database is unavailable: ${value}`);
  }
}

/** `DbError.Sql`: anything else SQLite refused: a syntax error, a missing table, more than one statement where one is expected. Construct and test it as `DbError.Sql`. */
export class DbErrorSql extends DbError {
  declare readonly kind: "sql";
  constructor(readonly message_: string) {
    super("sql", `SQL error: ${message_}`);
  }
}

/** `DbError.Migration`: a migration failed (everything the open migrated is rolled back), or the database is newer than the newest migration. Construct and test it as `DbError.Migration`. */
export class DbErrorMigration extends DbError {
  declare readonly kind: "migration";
  constructor(
    readonly version: number,
    readonly message_: string,
  ) {
    super("migration", `migration ${version} failed: ${message_}`);
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
