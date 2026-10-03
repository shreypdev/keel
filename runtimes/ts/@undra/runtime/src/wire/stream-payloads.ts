import { ReplyStatus, decodeAll, invalidTag } from "./payloads.js";
import { UndraReader } from "./reader.js";
import { UndraWriter } from "./writer.js";

/*
 * The failure of a stream (SPEC 3.7, ADR-036) and the flags of a stream item: what the stream support of a core reads
 * (`stream-support.ts`), apart from the framed transports' item encoder and decoder (`framed-payloads.ts`), so that an app whose
 * schema has a stream carries these few lines up front and not the transports' payload codecs (ADR-057).
 */


/** What a stream item message means (section 3.7, ADR-036). */
export enum StreamFlag {
  /** An item; `body` is the encoded `T`. */
  Item = 0,
  /** The stream ended normally. */
  End = 1,
  /**
   * The stream ended with **its own** typed error; `body` is the encoded `E`. Only a method whose
   * schema return is `Result<Stream<T>, E>` receives it.
   */
  Error = 2,
  /**
   * The call failed, in the reply-failure vocabulary: the body is a {@link StreamFailure} (a panic,
   * a cancellation by the core, a refusal). Ends the stream; needs no credit.
   */
  Failed = 3,
}

/** The reply statuses a {@link StreamFailure} may carry: panicked, cancelled by the core, refused. */
export type StreamFailureStatus = ReplyStatus.Panic | ReplyStatus.Cancelled | ReplyStatus.BadRequest;

/**
 * Body of a {@link StreamFlag.Failed} item (section 3.7, ADR-036): `status u8, message String,
 * detail String`. `status` is a section 3.4 reply status, so a failed stream maps exactly like a
 * failed reply with that status and the body {@link streamFailureReplyBody} builds.
 */
export interface StreamFailure {
  /** How the call failed. Any other status is rejected on decode (`invalid_tag`, `"StreamFailure.status"`). */
  readonly status: StreamFailureStatus;
  /** The panic message (status 2), the cancellation reason (3) or the refusal reason (5). */
  readonly message: string;
  /** The backtrace of a panic; empty otherwise. */
  readonly detail: string;
}

export function writeStreamFailure(w: UndraWriter, failure: StreamFailure): void {
  w.writeU8(failure.status);
  w.writeStr(failure.message);
  w.writeStr(failure.detail);
}

export function readStreamFailure(r: UndraReader): StreamFailure {
  const at = r.position;
  const status = r.readU8();
  if (status !== ReplyStatus.Panic && status !== ReplyStatus.Cancelled && status !== ReplyStatus.BadRequest) {
    throw invalidTag(status, at, "StreamFailure.status");
  }
  const message = r.readStr();
  return { status, message, detail: r.readStr() };
}

/** Encodes the body of a {@link StreamFlag.Failed} item. */
export function encodeStreamFailure(failure: StreamFailure): Uint8Array {
  const w = new UndraWriter(9 + failure.message.length + failure.detail.length);
  writeStreamFailure(w, failure);
  return w.finish();
}

/** Decodes the body of a {@link StreamFlag.Failed} item; the whole input must be consumed. */
export function decodeStreamFailure(bytes: Uint8Array): StreamFailure {
  return decodeAll(bytes, readStreamFailure);
}

/**
 * The section 3.4 reply body of a failed reply with the failure's status: `String message +
 * String backtrace` (the message and detail) for a panic, empty for a cancellation, `String
 * reason` (the message) for a refusal. `new UndraReplyError(failure.status, body)` is then the
 * same error a failed call reports.
 */
export function streamFailureReplyBody(failure: StreamFailure): Uint8Array {
  if (failure.status === ReplyStatus.Cancelled) return new Uint8Array(0);
  const w = new UndraWriter(8 + failure.message.length + failure.detail.length);
  w.writeStr(failure.message);
  if (failure.status === ReplyStatus.Panic) w.writeStr(failure.detail);
  return w.finish();
}
