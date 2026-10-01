import { UndraError } from "./base-error.js";
import { UndraReader, ReplyStatus } from "./wire/index.js";

/*
 * Typed errors of the runtime (docs/SPEC.md section 17.1). Everything the
 * runtime throws or rejects with is an `UndraError`, so `error instanceof
 * UndraError` separates Undra failures from programming errors, and `kind` is a
 * stable discriminant to switch on. Generated code subclasses `UndraError` for
 * the typed errors of a core (`TodoError` ...).
 */

export { UndraError };

/** Reads the `String` fields a panic or bad-request body carries; `undefined` when the body does not decode. */
function readStrings(body: Uint8Array, count: number): string[] | undefined {
  try {
    const r = new UndraReader(body);
    const out: string[] = [];
    for (let i = 0; i < count; i++) out.push(r.readStr());
    return out;
  } catch {
    return undefined;
  }
}

function describeReply(status: ReplyStatus, body: Uint8Array): string {
  switch (status) {
    case ReplyStatus.Error:
      return "the call failed with a typed error";
    case ReplyStatus.Panic: {
      const [message] = readStrings(body, 2) ?? [];
      return message === undefined ? "the core panicked" : `the core panicked: ${message}`;
    }
    case ReplyStatus.Cancelled:
      return "the call was cancelled";
    case ReplyStatus.BadRequest: {
      const [reason] = readStrings(body, 1) ?? [];
      return reason === undefined ? "the core rejected the request" : `the core rejected the request: ${reason}`;
    }
    default:
      return `unexpected reply status ${String(status)}`;
  }
}

/**
 * A call did not succeed. `status` says how (SPEC 3.4) and `body` is the raw
 * reply body: for {@link ReplyStatus.Error} the encoded `E` of the method's
 * `Result<T, E>` (generated code decodes it into its typed error), for
 * {@link ReplyStatus.Panic} the encoded `message` and `backtrace` strings, for
 * {@link ReplyStatus.BadRequest} the encoded reason string, and empty for
 * {@link ReplyStatus.Cancelled}. A failed stream reports its error the same
 * way: status {@link ReplyStatus.Error} with the encoded `E` when it ends with
 * its own typed error, and the status and body of the matching failed reply
 * when the core reports that the stream panicked, was cancelled by the core or
 * was refused (`StreamFlag.Failed`, ADR-036).
 */
export class UndraReplyError extends UndraError {
  override readonly name: string = "UndraReplyError";
  /** How the call ended. */
  readonly status: ReplyStatus;
  /** The reply body, undecoded. */
  readonly body: Uint8Array;

  /** @param status Outcome code of the reply. @param body The reply body. */
  constructor(status: ReplyStatus, body: Uint8Array) {
    super("reply", describeReply(status, body));
    this.status = status;
    this.body = body;
  }

  /** The panic message (status 2) or the bad-request reason (status 5); `undefined` for other statuses or an undecodable body. */
  get reason(): string | undefined {
    if (this.status === ReplyStatus.Panic) return readStrings(this.body, 2)?.[0];
    if (this.status === ReplyStatus.BadRequest) return readStrings(this.body, 1)?.[0];
    return undefined;
  }

  /** The core's backtrace of a panic (status 2); `undefined` otherwise. */
  get backtrace(): string | undefined {
    return this.status === ReplyStatus.Panic ? readStrings(this.body, 2)?.[1] : undefined;
  }
}

/** An operation is not available in the mode the core was loaded in (`callSync` outside `wasm-main`). */
export class UndraModeError extends UndraError {
  override readonly name: string = "UndraModeError";
  /** The operation that was attempted. */
  readonly operation: string;
  /** The mode the core runs in. */
  readonly mode: string;

  /** @param operation Name of the attempted operation. @param mode The mode of the core. */
  constructor(operation: string, mode: string) {
    super("mode", `${operation} is not available in mode '${mode}'`);
    this.operation = operation;
    this.mode = mode;
  }
}

function hex64(value: bigint): string {
  return `0x${value.toString(16).padStart(16, "0")}`;
}

/** The core was built from a different schema than the generated bindings expect (SPEC 11, R7). */
export class UndraSchemaMismatchError extends UndraError {
  override readonly name: string = "UndraSchemaMismatchError";
  /** The schema hash the bindings were generated from. */
  readonly expected: bigint;
  /** The schema hash the core reports. */
  readonly got: bigint;

  /** @param expected Hash the bindings expect. @param got Hash the core reported. */
  constructor(expected: bigint, got: bigint) {
    super(
      "schemaMismatch",
      `schema mismatch: the bindings expect ${hex64(expected)} but the core reports ${hex64(got)}; regenerate the bindings or rebuild the core`,
    );
    this.expected = expected;
    this.got = got;
  }
}

/**
 * The dev server no longer holds the objects of this core (ADR-051): it was restarted (`undra dev`
 * rebuilt the core) or the session's grace period passed while the client was away. The handles of
 * every store and object of this core are dead; load a new core and create them again. A core
 * reports this as `closed` with reason `"sessionLost"`; the page of a web app reloads.
 */
export class UndraSessionLostError extends UndraError {
  override readonly name: string = "UndraSessionLostError";

  /** @param message The server's reason, when it gave one. */
  constructor(message = "the dev server no longer has this core's objects (it was restarted, or the session expired); load a new core") {
    super("sessionLost", message);
  }
}

/**
 * Thrown by a generated port adapter when the implementation fails with the
 * port's typed error. `body` is the encoded error; the runtime answers the
 * port call with status 1 and this body.
 */
export class UndraPortError extends UndraError {
  override readonly name: string = "UndraPortError";
  /** The encoded typed error. */
  readonly body: Uint8Array;

  /** @param body The encoded error value. */
  constructor(body: Uint8Array) {
    super("port", "typed port failure");
    this.body = body;
  }
}

/** Why a transport failed; see {@link UndraTransportError}. */
export type TransportFailure =
  /** The core was closed, or the connection to it ended. */
  | "closed"
  /** The handshake (`Hello`, instantiation, `undra_init`) failed. */
  | "handshake"
  /** The wasm core trapped (a panic with `panic=abort`); the instance cannot be used again. */
  | "trap"
  /** The peer sent something that is not the protocol. */
  | "protocol"
  /** The peer did not answer in time. */
  | "timeout"
  /** The environment lacks something the transport needs (`WebSocket`, `Worker`, `WebAssembly`). */
  | "unsupported";

/** The channel to the core failed. In-flight calls and streams reject with this. */
export class UndraTransportError extends UndraError {
  override readonly name: string = "UndraTransportError";
  /** What went wrong. */
  readonly reason: TransportFailure;

  /** @param reason What went wrong. @param message Description. @param options Standard `Error` options. */
  constructor(reason: TransportFailure, message: string, options?: ErrorOptions) {
    super("transport", message, options);
    this.reason = reason;
  }
}

/** What each `undra_restore` code means, for the message of {@link UndraRestoreError}. */
const RESTORE_CODES: Readonly<Record<number, string>> = {
  2: "a store's restore panicked",
  5: "the snapshot is malformed or names something this core does not have",
  6: "the core is shut down, or the restore was made from inside a core callback",
  7: "a store's persisted values cannot become this build's types (see the core's error log)",
};

/**
 * `UndraCore.restore` was refused: the core rejected the snapshot and is unchanged (SPEC 5.9, the
 * `undra_restore` code of SPEC 7). A refused restore changes nothing, so the core and every handle
 * keep working. The Swift runtime has the same error (`UndraRestoreError`).
 */
export class UndraRestoreError extends UndraError {
  override readonly name: string = "UndraRestoreError";
  /** `undra_restore` code 2: a store's restore function panicked (contained). */
  static readonly PANICKED = 2;
  /** `undra_restore` code 5: the snapshot is malformed (also one in a layout before ADR-037), names an unknown store type, has a null or duplicate handle, or a store rejected its values. */
  static readonly BAD_SNAPSHOT = 5;
  /** `undra_restore` code 6: no running core, it is shut down, or the restore was made from inside a core callback. */
  static readonly UNAVAILABLE = 6;
  /**
   * `undra_restore` code 7 (ADR-037): a store's persisted values cannot become this build's types; they neither
   * migrate structurally nor through a `#[undra::migrate]` hook. The reason is in the ERROR record the core logged.
   */
  static readonly INCOMPATIBLE = 7;
  /**
   * The non-zero code `undra_restore` returned: {@link UndraRestoreError.PANICKED} (2) a store's restore panicked,
   * {@link UndraRestoreError.BAD_SNAPSHOT} (5) the snapshot is malformed or names something the core does not have,
   * {@link UndraRestoreError.UNAVAILABLE} (6) the core is shut down or was called from inside a callback,
   * {@link UndraRestoreError.INCOMPATIBLE} (7) a store's persisted values cannot become this build's types (ADR-037).
   */
  readonly code: number;

  /** @param code The non-zero code `undra_restore` returned. */
  constructor(code: number) {
    super(
      "restore",
      `the Undra core rejected the snapshot (code ${String(code)}${RESTORE_CODES[code] === undefined ? "" : `: ${RESTORE_CODES[code]}`}); a rejected restore leaves the core unchanged`,
    );
    this.code = code;
  }
}
