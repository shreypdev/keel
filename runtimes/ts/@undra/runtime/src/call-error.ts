import { UndraError } from "./base-error.js";
import {
  UndraModeError,
  UndraPortError,
  UndraReplyError,
  UndraRestoreError,
  UndraSchemaMismatchError,
  UndraSessionLostError,
  UndraTransportError,
} from "./errors.js";
import { errorMessage } from "./platform.js";
import { type Codec, ReplyStatus, WireError, decodeValue } from "./wire/index.js";

/*
 * What a generated call fails with besides its own error type and the abort of its caller (ADR-032,
 * amendment A, docs/SPEC.md 10.3 and 17.1), and the one place that maps the runtime's raw errors onto it.
 *
 * A generated method rejects with exactly one of three things: its own typed error `E` (reply status 1),
 * the reason of the `AbortSignal` that cancelled it (an `AbortError` by default), or an `UndraCallError`.
 * A generated command (a synchronous method that returns nothing and has no error type) never rejects:
 * it reports to `onError` instead.
 */

/** The discriminant of an {@link UndraCallError}: `error.kind`. */
export type UndraCallErrorKind = "cancelledByCore" | "panicked" | "refused" | "unavailable" | "malformed";

/**
 * Why a call into the core did not produce its result, when the reason is neither the method's own error
 * type nor the cancellation of the caller (ADR-032, amendment A). `kind` is the discriminant; the five
 * final classes in the `UndraCallError` namespace carry the details of their case.
 *
 * ```ts
 * try {
 *   await todos.add(draft);
 * } catch (error) {
 *   if (error instanceof TodoError) problem = "Give it a title first.";
 *   else if (error instanceof UndraCallError) problem = error.message; // panicked, refused, unavailable, ...
 *   else throw error;                                                  // an AbortError: the caller left
 * }
 * ```
 *
 * `switch (error.kind)` over an {@link UndraCallFailure} is exhaustive.
 */
export abstract class UndraCallError extends UndraError {
  declare readonly kind: UndraCallErrorKind;

  /**
   * The error a generated method without an error type rejects with for `error`, a failure of
   * `UndraCore.call` or `construct`, or of decoding their result: an abort reason and anything that is
   * not Undra's stay themselves; every failure of the runtime becomes an `UndraCallError`.
   *
   * | Raw failure | Result |
   * |---|---|
   * | an `UndraCallError`, an abort reason, any error that is not Undra's, a typed `E` | itself |
   * | `UndraReplyError` status 2 | `Panicked` |
   * | status 3 | `CancelledByCore` |
   * | status 5, `UndraModeError`, `UndraRestoreError` | `Refused` |
   * | status 1 (the method has no error type), 0, 4 | `Malformed` |
   * | `UndraTransportError` (closed, handshake, trap, timeout, unsupported, restarted; also what a `remote` core fails with while it reconnects, ADR-051), `UndraSchemaMismatchError`, `UndraSessionLostError` | `Unavailable` |
   * | `UndraTransportError("protocol")`, `WireError`, `UndraPortError`, an `UndraError` the runtime itself raised | `Malformed` |
   */
  static mapped(error: unknown): unknown;
  /**
   * The same for a method whose Rust signature returns `Result<_, E>`: a typed reply is decoded with
   * `domain` (the generated `<E>Codec`) and returned as `E`; a body that does not decode is `Malformed`.
   */
  static mapped<E>(error: unknown, domain: Codec<E>): unknown;
  static mapped(error: unknown, domain?: Codec<unknown>): unknown {
    if (domain !== undefined && error instanceof UndraReplyError && error.status === ReplyStatus.Error) {
      return decodeTyped(error.body, domain) ?? new UndraCallError.Malformed(`a typed error that does not decode (${error.body.length} bytes)`, { cause: error });
    }
    return classify(error);
  }

  /**
   * The error a generated stream method ends with, for a failure of `UndraCore.stream`.
   *
   * A stream fails in the vocabulary of a failed reply (ADR-036): a stream the core ended itself (a restore or a
   * shutdown) is `CancelledByCore`, one that panicked is `Panicked` with the message and backtrace, one the core
   * refused is `Refused`, exactly as for a call. Only a stream with an error type can end with a typed error
   * (reply status 1, the encoded `E`): with a `domain` (the generated `<E>Codec`) it is decoded and returned as
   * `E`, and a body that does not decode is `Malformed`; without one it is `Malformed`.
   */
  static mappedStream(error: unknown, domain?: Codec<unknown>): unknown {
    if (domain !== undefined) return UndraCallError.mapped(error, domain);
    if (error instanceof UndraReplyError && error.status === ReplyStatus.Error) {
      return new UndraCallError.Malformed(`a stream without an error type ended with a typed error item (${error.body.length} bytes)`, {
        cause: error,
      });
    }
    return classify(error);
  }

  /** `error` as the {@link UndraCallError} a report carries: what {@link UndraCallError.mapped} returns, or `Malformed` for a failure that is not Undra's. */
  static asCallError(error: unknown): UndraCallError {
    const mapped = classify(error);
    if (mapped instanceof UndraCallError) return mapped;
    const text = errorMessage(error);
    return new UndraCallError.Malformed(text, { cause: error }, `an unexpected failure: ${text}`);
  }
}

/** One class per case of {@link UndraCallError}. */
export namespace UndraCallError {
  /**
   * The core cancelled the call: a restore replaced or invalidated the object it ran on, or the core
   * shut down while it ran (reply status 3). The caller's own `AbortSignal` rejects with its reason
   * instead.
   */
  export class CancelledByCore extends UndraCallError {
    override readonly name: string = "UndraCallError.CancelledByCore";
    declare readonly kind: "cancelledByCore";

    /** @param options Standard `Error` options. */
    constructor(options?: ErrorOptions) {
      super("cancelledByCore", "the Undra core cancelled the call (a restore replaced its object, or the core shut down)", options);
    }
  }

  /** The core panicked while running the call (reply status 2). The core caught the panic and keeps working (a wasm core traps instead: `Unavailable`). */
  export class Panicked extends UndraCallError {
    override readonly name: string = "UndraCallError.Panicked";
    declare readonly kind: "panicked";
    /** The panic message. */
    readonly panicMessage: string;
    /** The core's backtrace; empty for a stream panic. */
    readonly backtrace: string;

    /** @param panicMessage The panic message. @param backtrace The backtrace. @param options Standard `Error` options. */
    constructor(panicMessage: string, backtrace: string, options?: ErrorOptions) {
      super("panicked", `the Undra core panicked: ${panicMessage}`, options);
      this.panicMessage = panicMessage;
      this.backtrace = backtrace;
    }
  }

  /**
   * The core refused the call without running it (reply status 5): the object was closed or replaced
   * by a restore, the call was made from inside one of the core's callbacks (`E_REENTRANT`), or the
   * request could not be decoded.
   */
  export class Refused extends UndraCallError {
    override readonly name: string = "UndraCallError.Refused";
    declare readonly kind: "refused";
    /** The core's text. */
    readonly reason: string;

    /** @param reason The core's text. @param options Standard `Error` options. */
    constructor(reason: string, options?: ErrorOptions) {
      super("refused", `the Undra core refused the call: ${reason}`, options);
      this.reason = reason;
    }
  }

  /**
   * The core cannot be reached: it was closed, trapped or never loaded, or the connection to it closed or timed out.
   * A wasm core loaded with `recovery` that trapped and restarted fails what was in flight with reason `"restarted"`
   * (ADR-049): the call may or may not have run, and it is not retried.
   * Over `undra dev` it is also what every call rejects with while the connection is down and `UndraCore.connection`
   * is `reconnecting` (and what was in flight when it dropped rejects with): the connection state says what the runtime
   * is doing about it, and a command's failure of that kind is not handed to `onError`.
   */
  export class Unavailable extends UndraCallError {
    override readonly name: string = "UndraCallError.Unavailable";
    declare readonly kind: "unavailable";
    /** What happened; also the `cause`. */
    readonly transport: UndraTransportError;

    /** @param transport What happened. */
    constructor(transport: UndraTransportError) {
      super("unavailable", `the Undra core is unavailable: ${transport.message}`, { cause: transport });
      this.transport = transport;
    }
  }

  /** The core answered with something the bindings cannot read. After a successful schema check this is a bug in Undra: please report it with the text. */
  export class Malformed extends UndraCallError {
    override readonly name: string = "UndraCallError.Malformed";
    declare readonly kind: "malformed";
    /** What could not be read. */
    readonly detail: string;

    /**
     * @param detail What could not be read.
     * @param options Standard `Error` options.
     * @param message Replaces the default description (used for a failure that is not a reply at all).
     */
    constructor(detail: string, options?: ErrorOptions, message?: string) {
      super("malformed", message ?? `the Undra core sent a reply the bindings cannot read: ${detail}`, options);
      this.detail = detail;
    }
  }
}

/** The five cases of {@link UndraCallError}, as a union: `switch (error.kind)` over it is exhaustive and narrows to the case's class. */
export type UndraCallFailure =
  | UndraCallError.CancelledByCore
  | UndraCallError.Panicked
  | UndraCallError.Refused
  | UndraCallError.Unavailable
  | UndraCallError.Malformed;

/**
 * A failure no caller could see: a generated command (a synchronous method that returns nothing and has
 * no error type), a store's change that could not be applied, a malformed change-set, a failed port.
 * Delivered to `onError` by `UndraCore.report` (ADR-032, amendment A).
 */
export class UndraUnhandledError extends UndraError {
  override readonly name: string = "UndraUnhandledError";
  /** What failed, as TypeScript spells it: `"Todos.toggle"`, `"configureRemote"`, `"Todos.apply(signal: 2)"`. */
  readonly operation: string;
  /** Why. */
  readonly error: UndraCallError;

  /** @param operation What failed. @param error Why. @param cause The original failure, when it was not an Undra one. */
  constructor(operation: string, error: UndraCallError, cause?: unknown) {
    super("unhandled", `${operation} failed: ${error.message}`, { cause: cause ?? error });
    this.operation = operation;
    this.error = error;
  }
}

// ----- the mapping ---------------------------------------------------------------------------

/** The single place every failure goes through. */
function classify(error: unknown): unknown {
  if (error instanceof UndraCallError) return error;
  if (error instanceof UndraReplyError) return fromReply(error);
  if (error instanceof UndraTransportError) {
    // "protocol" is the peer breaking the protocol, not the core being out of reach.
    return error.reason === "protocol" ? new UndraCallError.Malformed(error.message, { cause: error }) : new UndraCallError.Unavailable(error);
  }
  if (error instanceof UndraSchemaMismatchError || error instanceof UndraSessionLostError) {
    // A remote core that came back with another schema (`undra dev` rebuilt it) or without this core's objects: the
    // connection is closed for good and every call in flight fails with this.
    return new UndraCallError.Unavailable(new UndraTransportError("closed", error.message, { cause: error }));
  }
  if (error instanceof WireError) return new UndraCallError.Malformed(`the reply does not decode: ${error.message}`, { cause: error });
  if (error instanceof UndraModeError || error instanceof UndraRestoreError) return new UndraCallError.Refused(error.message, { cause: error });
  if (error instanceof UndraPortError) return new UndraCallError.Malformed("a port implementation's typed failure reached a call", { cause: error });
  // What the runtime itself raised without a class of its own (`UndraError("state" | "options" | "observe")`) is a
  // misuse or a bug it cannot classify; a typed `E` (a subclass) and everything that is not Undra's pass through.
  if (error instanceof UndraError && error.constructor === UndraError) return new UndraCallError.Malformed(error.message, { cause: error });
  return error;
}

/** The mapping of a reply that is not the method's own error. */
function fromReply(error: UndraReplyError): UndraCallError {
  switch (error.status) {
    case ReplyStatus.Cancelled:
      return new UndraCallError.CancelledByCore({ cause: error });
    case ReplyStatus.Panic:
      return new UndraCallError.Panicked(error.reason ?? "<undecodable panic report>", error.backtrace ?? "", { cause: error });
    case ReplyStatus.BadRequest:
      return new UndraCallError.Refused(error.reason ?? "<undecodable reason>", { cause: error });
    case ReplyStatus.Error:
      return new UndraCallError.Malformed(`the core answered with a typed error, but this method has none (${error.body.length} bytes)`, { cause: error });
    case ReplyStatus.StreamOpened:
      return new UndraCallError.Malformed("the core opened a stream where a single reply was expected", { cause: error });
    default:
      return new UndraCallError.Malformed(`the core answered with status ${String(error.status)} as a failure`, { cause: error });
  }
}

/** A typed reply body decoded as `E`, or `undefined` when it does not decode. */
function decodeTyped(body: Uint8Array, domain: Codec<unknown>): unknown {
  try {
    return decodeValue(domain, body);
  } catch {
    return undefined;
  }
}
