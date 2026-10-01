import type { Codec } from "./codec.js";
import { PatchError, WireError } from "./errors.js";
import { UndraReader } from "./reader.js";
import type { Handle } from "./types.js";
import { UndraWriter } from "./writer.js";

/*
 * Typed encoders and decoders for every envelope payload (docs/SPEC.md
 * sections 3.3 to 3.8 and 5.9). `encodeX` produces exactly the payload bytes
 * (the envelope header is added by `encodeEnvelope`); `decodeX` requires the
 * whole input to be consumed.
 *
 * Several payloads end with an opaque tail (call arguments, a reply body, a
 * stream item). Those tails stay as bytes here: they are encoded and decoded
 * per method by generated code with the codecs of `./codec.js`. Decoded tails
 * are borrowed views into the decoded buffer (see `UndraReader.readBytes`).
 */

/** Sentinel `signalId` meaning "every signal of the store" (`u32::MAX`). */
export const ALL_SIGNALS = 0xffff_ffff;

function invalidTag(tag: number, at: number, ty: string): WireError {
  return new WireError({ code: "invalid_tag", tag, at, ty });
}

/** Decodes a whole payload with `read` and asserts nothing is left over. */
function decodeAll<T>(bytes: Uint8Array, read: (r: UndraReader) => T): T {
  const r = new UndraReader(bytes);
  const value = read(r);
  r.finish();
  return value;
}

/** Decodes a payload whose last field is an opaque tail consumed by `read` via `readRest`. */
function decodeWithTail<T>(bytes: Uint8Array, read: (r: UndraReader) => T): T {
  return read(new UndraReader(bytes));
}

// ---------------------------------------------------------------------------
// Call (kind 1)
// ---------------------------------------------------------------------------

/** What a call is addressed to (section 3.3). */
export enum CallTarget {
  /** A free function (`fn.<name>`). */
  FreeFunction = 0,
  /** A method of the object behind `handle`. */
  ObjectMethod = 1,
  /** An object constructor; `typeId` names the object type. */
  Constructor = 2,
  /** One page of a lazy list object. */
  LazyListPage = 3,
}

/**
 * Payload of a `Call`. The four variants have different layouts:
 * `target u8, handle u64, method_id u32, call_id u32, args` for functions
 * (the handle is written as 0 and ignored on decode) and methods;
 * `target u8, type_id u32, method_id u32, call_id u32, args` for constructors;
 * `target u8, handle u64, offset u32, limit u32, call_id u32` for lazy pages.
 * `args` are the encoded parameters in declaration order.
 */
export type CallPayload =
  | {
      readonly target: CallTarget.FreeFunction;
      readonly methodId: number;
      readonly callId: number;
      readonly args: Uint8Array;
    }
  | {
      readonly target: CallTarget.ObjectMethod;
      readonly handle: Handle;
      readonly methodId: number;
      readonly callId: number;
      readonly args: Uint8Array;
    }
  | {
      readonly target: CallTarget.Constructor;
      readonly typeId: number;
      readonly methodId: number;
      readonly callId: number;
      readonly args: Uint8Array;
    }
  | {
      readonly target: CallTarget.LazyListPage;
      readonly handle: Handle;
      readonly offset: number;
      readonly limit: number;
      readonly callId: number;
    };

/** Encodes a `Call` payload. */
export function encodeCall(call: CallPayload): Uint8Array {
  const w = new UndraWriter(call.target === CallTarget.LazyListPage ? 21 : 17 + call.args.length);
  w.writeU8(call.target);
  switch (call.target) {
    case CallTarget.FreeFunction:
      w.writeU64(0n);
      w.writeU32(call.methodId);
      w.writeU32(call.callId);
      w.writeRaw(call.args);
      break;
    case CallTarget.ObjectMethod:
      w.writeU64(call.handle);
      w.writeU32(call.methodId);
      w.writeU32(call.callId);
      w.writeRaw(call.args);
      break;
    case CallTarget.Constructor:
      w.writeU32(call.typeId);
      w.writeU32(call.methodId);
      w.writeU32(call.callId);
      w.writeRaw(call.args);
      break;
    case CallTarget.LazyListPage:
      w.writeU64(call.handle);
      w.writeU32(call.offset);
      w.writeU32(call.limit);
      w.writeU32(call.callId);
      break;
  }
  return w.finish();
}

/** Decodes a `Call` payload. `args` is a borrowed view into `bytes`. */
export function decodeCall(bytes: Uint8Array): CallPayload {
  const r = new UndraReader(bytes);
  const at = r.position;
  const target = r.readU8();
  switch (target) {
    case CallTarget.FreeFunction: {
      r.readU64();
      const methodId = r.readU32();
      const callId = r.readU32();
      return { target, methodId, callId, args: r.readRest() };
    }
    case CallTarget.ObjectMethod: {
      const handle = r.readU64();
      const methodId = r.readU32();
      const callId = r.readU32();
      return { target, handle, methodId, callId, args: r.readRest() };
    }
    case CallTarget.Constructor: {
      const typeId = r.readU32();
      const methodId = r.readU32();
      const callId = r.readU32();
      return { target, typeId, methodId, callId, args: r.readRest() };
    }
    case CallTarget.LazyListPage: {
      const handle = r.readU64();
      const offset = r.readU32();
      const limit = r.readU32();
      const callId = r.readU32();
      r.finish();
      return { target, handle, offset, limit, callId };
    }
    default:
      throw invalidTag(target, at, "CallTarget");
  }
}

// ---------------------------------------------------------------------------
// Reply (kind 2)
// ---------------------------------------------------------------------------

/** Outcome code of a reply (section 3.4). */
export enum ReplyStatus {
  /** The call returned; `body` is the return value (empty for `Unit`, the `T` of a `Result<T, E>`). */
  Ok = 0,
  /** The call returned the typed error `E` of a `Result<T, E>`; `body` is the encoded `E`. */
  Error = 1,
  /** The core panicked inside the call. */
  Panic = 2,
  /** The call was cancelled. */
  Cancelled = 3,
  /** The call opened a stream; items follow as `StreamItem` messages. */
  StreamOpened = 4,
  /** The core could not serve the request (unknown method, undecodable arguments, schema mismatch, stale handle). */
  BadRequest = 5,
}

/** Payload of a `Reply`. `Ok` and `Error` bodies are encoded values, decoded per method by generated code. */
export type ReplyPayload =
  | { readonly callId: number; readonly status: ReplyStatus.Ok | ReplyStatus.Error; readonly body: Uint8Array }
  | {
      readonly callId: number;
      readonly status: ReplyStatus.Panic;
      readonly message: string;
      readonly backtrace: string;
    }
  | { readonly callId: number; readonly status: ReplyStatus.Cancelled | ReplyStatus.StreamOpened }
  | { readonly callId: number; readonly status: ReplyStatus.BadRequest; readonly reason: string };

/** Encodes a `Reply` payload. */
export function encodeReply(reply: ReplyPayload): Uint8Array {
  const w = new UndraWriter(
    reply.status === ReplyStatus.Ok || reply.status === ReplyStatus.Error ? 5 + reply.body.length : 64,
  );
  w.writeU32(reply.callId);
  w.writeU8(reply.status);
  switch (reply.status) {
    case ReplyStatus.Ok:
    case ReplyStatus.Error:
      w.writeRaw(reply.body);
      break;
    case ReplyStatus.Panic:
      w.writeStr(reply.message);
      w.writeStr(reply.backtrace);
      break;
    case ReplyStatus.Cancelled:
    case ReplyStatus.StreamOpened:
      break;
    case ReplyStatus.BadRequest:
      w.writeStr(reply.reason);
      break;
  }
  return w.finish();
}

/** Decodes a `Reply` payload. An `Ok` or `Error` body is a borrowed view into `bytes`. */
export function decodeReply(bytes: Uint8Array): ReplyPayload {
  const r = new UndraReader(bytes);
  const callId = r.readU32();
  const at = r.position;
  const status = r.readU8();
  switch (status) {
    case ReplyStatus.Ok:
    case ReplyStatus.Error:
      return { callId, status, body: r.readRest() };
    case ReplyStatus.Panic: {
      const message = r.readStr();
      const backtrace = r.readStr();
      r.finish();
      return { callId, status, message, backtrace };
    }
    case ReplyStatus.Cancelled:
    case ReplyStatus.StreamOpened:
      r.finish();
      return { callId, status };
    case ReplyStatus.BadRequest: {
      const reason = r.readStr();
      r.finish();
      return { callId, status, reason };
    }
    default:
      throw invalidTag(status, at, "ReplyStatus");
  }
}

// ---------------------------------------------------------------------------
// ChangeSet (kind 3)
// ---------------------------------------------------------------------------

/** How a change-set entry's `value` is to be interpreted (section 3.5). */
export enum ChangeOp {
  /** `value` is the signal's whole new value, encoded as its `T`. */
  FullValue = 0,
  /** `value` is a keyed patch (see {@link decodePatch}) to apply to the current list. */
  KeyedPatch = 1,
  /** A lazy list changed; `value` is empty and the host re-pages. */
  LazyInvalidated = 2,
}

/** One signal update inside a change-set. `value` is a borrowed view into the decoded buffer. */
export interface ChangeEntry {
  /** The store the signal belongs to. */
  readonly handle: Handle;
  /** Index of the signal within the store. */
  readonly signalId: number;
  /** How to interpret `value`. */
  readonly op: ChangeOp;
  /** Encoded value or patch; empty for {@link ChangeOp.LazyInvalidated}. */
  readonly value: Uint8Array;
}

/** Payload of a `ChangeSet`: the signal updates of one transaction. */
export interface ChangeSetPayload {
  /** Monotonic transaction number. */
  readonly txnId: bigint;
  /** Updates in the order the core produced them. */
  readonly entries: readonly ChangeEntry[];
}

/** Fixed part of an entry: handle 8, signal id 4, op 1, len 4. */
const CHANGE_ENTRY_MIN = 17;

/** Encodes a `ChangeSet` payload. */
export function encodeChangeSet(changeSet: ChangeSetPayload): Uint8Array {
  let size = 12;
  for (const e of changeSet.entries) size += CHANGE_ENTRY_MIN + e.value.length;
  const w = new UndraWriter(size);
  w.writeU64(changeSet.txnId);
  w.writeLen(changeSet.entries.length);
  for (const e of changeSet.entries) {
    w.writeU64(e.handle);
    w.writeU32(e.signalId);
    w.writeU8(e.op);
    w.writeBytes(e.value);
  }
  return w.finish();
}

function readChangeEntry(r: UndraReader): ChangeEntry {
  const handle = r.readU64();
  const signalId = r.readU32();
  const at = r.position;
  const op = r.readU8();
  if (op > ChangeOp.LazyInvalidated) throw invalidTag(op, at, "ChangeOp");
  return { handle, signalId, op: op as ChangeOp, value: r.readBytes() };
}

/** Header of a change-set: its transaction id and entry count. */
export interface ChangeSetHeader {
  /** Monotonic transaction number. */
  readonly txnId: bigint;
  /** Number of entries that follow. */
  readonly count: number;
}

/**
 * Reads only the header of a change-set. The count is checked against the
 * input size (an entry is at least 17 bytes), so it is safe to size buffers
 * from it.
 */
export function readChangeSetHeader(bytes: Uint8Array): ChangeSetHeader {
  const r = new UndraReader(bytes);
  const txnId = r.readU64();
  return { txnId, count: r.readLen(CHANGE_ENTRY_MIN) };
}

/**
 * Lazily yields the entries of a change-set without copying their values:
 * each `value` is a view into `bytes`. Nothing is decoded beyond the entry
 * being yielded, so a consumer can stop early or skip entries it does not
 * observe.
 *
 * A change-set is a transaction and must be applied whole. Malformed input
 * throws when the generator reaches the bad entry (and trailing bytes after
 * the last one), possibly after earlier entries were yielded, so collect the
 * entries (or use {@link decodeChangeSet}) before applying any of them.
 */
export function* iterateChangeSet(bytes: Uint8Array): Generator<ChangeEntry, void, undefined> {
  const r = new UndraReader(bytes);
  r.readU64();
  const count = r.readLen(CHANGE_ENTRY_MIN);
  for (let i = 0; i < count; i++) yield readChangeEntry(r);
  r.finish();
}

/** Decodes and validates a whole `ChangeSet` payload; entry values are borrowed views into `bytes`. */
export function decodeChangeSet(bytes: Uint8Array): ChangeSetPayload {
  return decodeAll(bytes, (r) => {
    const txnId = r.readU64();
    const count = r.readLen(CHANGE_ENTRY_MIN);
    const entries = new Array<ChangeEntry>(count);
    for (let i = 0; i < count; i++) entries[i] = readChangeEntry(r);
    return { txnId, entries };
  });
}

// ---------------------------------------------------------------------------
// PortCall (kind 4) and PortReply (kind 5)
// ---------------------------------------------------------------------------

/** Payload of a `PortCall`: the core asks the platform to run a port method. */
export interface PortCallPayload {
  /** Port being called (`fnv1a32("port.<Trait>")`). */
  readonly portId: number;
  /** Method being called (`fnv1a32("<Trait>.<method>")`). */
  readonly methodId: number;
  /** Chosen by the core, unique among in-flight port calls. */
  readonly portCallId: number;
  /** Encoded parameters in declaration order. */
  readonly args: Uint8Array;
}

/** Encodes a `PortCall` payload. */
export function encodePortCall(call: PortCallPayload): Uint8Array {
  const w = new UndraWriter(12 + call.args.length);
  w.writeU32(call.portId);
  w.writeU32(call.methodId);
  w.writeU32(call.portCallId);
  w.writeRaw(call.args);
  return w.finish();
}

/** Decodes a `PortCall` payload. `args` is a borrowed view into `bytes`. */
export function decodePortCall(bytes: Uint8Array): PortCallPayload {
  return decodeWithTail(bytes, (r) => {
    const portId = r.readU32();
    const methodId = r.readU32();
    const portCallId = r.readU32();
    return { portId, methodId, portCallId, args: r.readRest() };
  });
}

/** Outcome code of a port reply (section 3.6). */
export enum PortStatus {
  /** The port method returned; `body` is the return value. */
  Ok = 0,
  /** The port method returned its typed error; `body` is the encoded error. */
  Error = 1,
  /** The platform has no implementation of the port; `body` is empty. */
  Unavailable = 2,
}

/** Payload of a `PortReply`: the platform answers a `PortCall`. */
export interface PortReplyPayload {
  /** The `portCallId` being answered. */
  readonly portCallId: number;
  /** Outcome. */
  readonly status: PortStatus;
  /** Encoded return value or error; empty for {@link PortStatus.Unavailable}. */
  readonly body: Uint8Array;
}

/** Encodes a `PortReply` payload. */
export function encodePortReply(reply: PortReplyPayload): Uint8Array {
  const w = new UndraWriter(5 + reply.body.length);
  w.writeU32(reply.portCallId);
  w.writeU8(reply.status);
  w.writeRaw(reply.body);
  return w.finish();
}

/** Decodes a `PortReply` payload. `body` is a borrowed view into `bytes`. */
export function decodePortReply(bytes: Uint8Array): PortReplyPayload {
  return decodeWithTail(bytes, (r) => {
    const portCallId = r.readU32();
    const at = r.position;
    const status = r.readU8();
    if (status > PortStatus.Unavailable) throw invalidTag(status, at, "PortStatus");
    return { portCallId, status: status as PortStatus, body: r.readRest() };
  });
}

// ---------------------------------------------------------------------------
// Cancel (6), StreamCredit (7), StreamItem (8)
// ---------------------------------------------------------------------------

/** Payload of a `Cancel`: stop the in-flight call or stream `callId`. */
export interface CancelPayload {
  /** The call to cancel. */
  readonly callId: number;
}

/** Encodes a `Cancel` payload. */
export function encodeCancel(cancel: CancelPayload): Uint8Array {
  const w = new UndraWriter(4);
  w.writeU32(cancel.callId);
  return w.finish();
}

/** Decodes a `Cancel` payload. */
export function decodeCancel(bytes: Uint8Array): CancelPayload {
  return decodeAll(bytes, (r) => ({ callId: r.readU32() }));
}

/** Payload of a `StreamCredit`: the host allows `credit` more items on stream `callId`. */
export interface StreamCreditPayload {
  /** The stream. */
  readonly callId: number;
  /** Additional items the core may send. */
  readonly credit: number;
}

/** Encodes a `StreamCredit` payload. */
export function encodeStreamCredit(credit: StreamCreditPayload): Uint8Array {
  const w = new UndraWriter(8);
  w.writeU32(credit.callId);
  w.writeU32(credit.credit);
  return w.finish();
}

/** Decodes a `StreamCredit` payload. */
export function decodeStreamCredit(bytes: Uint8Array): StreamCreditPayload {
  return decodeAll(bytes, (r) => {
    const callId = r.readU32();
    return { callId, credit: r.readU32() };
  });
}

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

function writeStreamFailure(w: UndraWriter, failure: StreamFailure): void {
  w.writeU8(failure.status);
  w.writeStr(failure.message);
  w.writeStr(failure.detail);
}

function readStreamFailure(r: UndraReader): StreamFailure {
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

/** Payload of a `StreamItem`. */
export type StreamItemPayload =
  | { readonly callId: number; readonly flag: StreamFlag.Item | StreamFlag.Error; readonly body: Uint8Array }
  | { readonly callId: number; readonly flag: StreamFlag.End }
  | { readonly callId: number; readonly flag: StreamFlag.Failed; readonly failure: StreamFailure };

/** Encodes a `StreamItem` payload. */
export function encodeStreamItem(item: StreamItemPayload): Uint8Array {
  const w = new UndraWriter(
    item.flag === StreamFlag.End ? 5 : item.flag === StreamFlag.Failed ? 64 : 5 + item.body.length,
  );
  w.writeU32(item.callId);
  w.writeU8(item.flag);
  switch (item.flag) {
    case StreamFlag.Item:
    case StreamFlag.Error:
      w.writeRaw(item.body);
      break;
    case StreamFlag.End:
      break;
    case StreamFlag.Failed:
      writeStreamFailure(w, item.failure);
      break;
  }
  return w.finish();
}

/** Decodes a `StreamItem` payload. An item or error body is a borrowed view into `bytes`; a failure is decoded whole. */
export function decodeStreamItem(bytes: Uint8Array): StreamItemPayload {
  const r = new UndraReader(bytes);
  const callId = r.readU32();
  const at = r.position;
  const flag = r.readU8();
  switch (flag) {
    case StreamFlag.Item:
    case StreamFlag.Error:
      return { callId, flag, body: r.readRest() };
    case StreamFlag.End:
      r.finish();
      return { callId, flag };
    case StreamFlag.Failed: {
      const failure = readStreamFailure(r);
      r.finish();
      return { callId, flag, failure };
    }
    default:
      throw invalidTag(flag, at, "StreamFlag");
  }
}

// ---------------------------------------------------------------------------
// Observe (9), Release (10), Event (11)
// ---------------------------------------------------------------------------

/** Payload of an `Observe`: start (`on`) or stop observing a signal of a store. */
export interface ObservePayload {
  /** The store. */
  readonly handle: Handle;
  /** The signal, or {@link ALL_SIGNALS}. */
  readonly signalId: number;
  /** `true` to start observing, `false` to stop. */
  readonly on: boolean;
}

/** Encodes an `Observe` payload. */
export function encodeObserve(observe: ObservePayload): Uint8Array {
  const w = new UndraWriter(13);
  w.writeU64(observe.handle);
  w.writeU32(observe.signalId);
  w.writeBool(observe.on);
  return w.finish();
}

/** Decodes an `Observe` payload. */
export function decodeObserve(bytes: Uint8Array): ObservePayload {
  return decodeAll(bytes, (r) => {
    const handle = r.readU64();
    const signalId = r.readU32();
    return { handle, signalId, on: r.readBool() };
  });
}

/** Payload of a `Release`: the host drops its reference to an object. */
export interface ReleasePayload {
  /** The object to release. */
  readonly handle: Handle;
}

/** Encodes a `Release` payload. */
export function encodeRelease(release: ReleasePayload): Uint8Array {
  const w = new UndraWriter(8);
  w.writeU64(release.handle);
  return w.finish();
}

/** Decodes a `Release` payload. */
export function decodeRelease(bytes: Uint8Array): ReleasePayload {
  return decodeAll(bytes, (r) => ({ handle: r.readU64() }));
}

/** Payload of an `Event`: the platform reports something on an event port (connectivity, lifecycle). */
export interface EventPayload {
  /** The event port. */
  readonly portId: number;
  /** The port method being invoked. */
  readonly methodId: number;
  /** Encoded event parameters. */
  readonly payload: Uint8Array;
}

/** Encodes an `Event` payload. */
export function encodeEvent(event: EventPayload): Uint8Array {
  const w = new UndraWriter(8 + event.payload.length);
  w.writeU32(event.portId);
  w.writeU32(event.methodId);
  w.writeRaw(event.payload);
  return w.finish();
}

/** Decodes an `Event` payload. `payload` is a borrowed view into `bytes`. */
export function decodeEvent(bytes: Uint8Array): EventPayload {
  return decodeWithTail(bytes, (r) => {
    const portId = r.readU32();
    const methodId = r.readU32();
    return { portId, methodId, payload: r.readRest() };
  });
}

// ---------------------------------------------------------------------------
// Hello (12), Log (13), TimerFired (14)
// ---------------------------------------------------------------------------

/** Payload of a `Hello`, the handshake each side sends first. */
export interface HelloPayload {
  /** Version of the Undra crate or runtime that sent it. */
  readonly undraVersion: string;
  /** Schema hash of the sender; a mismatch is `UndraSchemaMismatch`. */
  readonly schemaHash: bigint;
  /** Platform name, for example `"web"`, `"ios"`, `"android"`, `"node"`. */
  readonly platform: string;
  /** Operating mode, for example `"prod"` or `"dev"`. */
  readonly mode: string;
}

/** Encodes a `Hello` payload. */
export function encodeHello(hello: HelloPayload): Uint8Array {
  const w = new UndraWriter(32);
  w.writeStr(hello.undraVersion);
  w.writeU64(hello.schemaHash);
  w.writeStr(hello.platform);
  w.writeStr(hello.mode);
  return w.finish();
}

/** Decodes a `Hello` payload. */
export function decodeHello(bytes: Uint8Array): HelloPayload {
  return decodeAll(bytes, (r) => {
    const undraVersion = r.readStr();
    const schemaHash = r.readU64();
    const platform = r.readStr();
    const mode = r.readStr();
    return { undraVersion, schemaHash, platform, mode };
  });
}

/** Payload of a `Log`: a record emitted by the core. */
export interface LogPayload {
  /** Severity as defined by the `Log` port (`u8`). */
  readonly level: number;
  /** Origin of the record. */
  readonly target: string;
  /** The message. */
  readonly message: string;
}

/** Encodes a `Log` payload. */
export function encodeLog(log: LogPayload): Uint8Array {
  const w = new UndraWriter(16 + log.target.length + log.message.length);
  w.writeU8(log.level);
  w.writeStr(log.target);
  w.writeStr(log.message);
  return w.finish();
}

/** Decodes a `Log` payload. */
export function decodeLog(bytes: Uint8Array): LogPayload {
  return decodeAll(bytes, (r) => {
    const level = r.readU8();
    const target = r.readStr();
    return { level, target, message: r.readStr() };
  });
}

/** Payload of a `TimerFired`: the timer set through the Timer port is due. */
export interface TimerFiredPayload {
  /** The id given to `Timer.set`. */
  readonly timerId: number;
}

/** Encodes a `TimerFired` payload. */
export function encodeTimerFired(timer: TimerFiredPayload): Uint8Array {
  const w = new UndraWriter(4);
  w.writeU32(timer.timerId);
  return w.finish();
}

/** Decodes a `TimerFired` payload. */
export function decodeTimerFired(bytes: Uint8Array): TimerFiredPayload {
  return decodeAll(bytes, (r) => ({ timerId: r.readU32() }));
}

// ---------------------------------------------------------------------------
// Snapshot (15) and Restore (16)
// ---------------------------------------------------------------------------

/** One stored signal value inside a snapshot. `value` is a borrowed view into the decoded buffer. */
export interface SnapshotSignal {
  /** Index of the signal within the store. */
  readonly signalId: number;
  /** The signal's encoded value. */
  readonly value: Uint8Array;
}

/** The signals of one store inside a snapshot. */
export interface SnapshotStore {
  /** The store's handle, re-issued unchanged on restore. */
  readonly handle: Handle;
  /** The store's type. */
  readonly typeId: number;
  /** Stored (non-computed) signals. */
  readonly signals: readonly SnapshotSignal[];
}

/**
 * Payload of a `Snapshot` (core to host) and of a `Restore` (host to core);
 * both use the layout of section 5.9.
 */
export interface SnapshotPayload {
  /**
   * The highest handle generation the core had issued when the snapshot was taken. A restore
   * resumes the core's generation counter above it, so no handle issued before the snapshot (or
   * between it and the restore) is ever issued again to another object (ADR-022). Opaque to the
   * host: pass it back unchanged.
   */
  readonly generationFloor: number;
  /** Every store in the snapshot. */
  readonly stores: readonly SnapshotStore[];
}

/** Smallest encoded store: handle 8, type id 4, signal count 4. */
const SNAPSHOT_STORE_MIN = 16;
/** Smallest encoded signal: signal id 4, len 4. */
const SNAPSHOT_SIGNAL_MIN = 8;

/** Encodes a `Snapshot` or `Restore` payload. */
export function encodeSnapshot(snapshot: SnapshotPayload): Uint8Array {
  const w = new UndraWriter();
  w.writeLen(snapshot.stores.length);
  w.writeU32(snapshot.generationFloor);
  for (const store of snapshot.stores) {
    w.writeU64(store.handle);
    w.writeU32(store.typeId);
    w.writeLen(store.signals.length);
    for (const signal of store.signals) {
      w.writeU32(signal.signalId);
      w.writeBytes(signal.value);
    }
  }
  return w.finish();
}

/** Decodes a `Snapshot` or `Restore` payload; signal values are borrowed views into `bytes`. */
export function decodeSnapshot(bytes: Uint8Array): SnapshotPayload {
  return decodeAll(bytes, (r) => {
    const storeCount = r.readLen(SNAPSHOT_STORE_MIN);
    const generationFloor = r.readU32();
    const stores = new Array<SnapshotStore>(storeCount);
    for (let i = 0; i < storeCount; i++) {
      const handle = r.readU64();
      const typeId = r.readU32();
      const signalCount = r.readLen(SNAPSHOT_SIGNAL_MIN);
      const signals = new Array<SnapshotSignal>(signalCount);
      for (let j = 0; j < signalCount; j++) {
        const signalId = r.readU32();
        signals[j] = { signalId, value: r.readBytes() };
      }
      stores[i] = { handle, typeId, signals };
    }
    return { generationFloor, stores };
  });
}

// ---------------------------------------------------------------------------
// Keyed patch (section 3.8)
// ---------------------------------------------------------------------------

/**
 * One operation of a keyed patch on a `Signal<Vec<T>>`. Operations apply in
 * order and every index refers to the list as left by the previous operation.
 *
 * - `insert`: put `item` at `index` (`0..=length`), shifting later items up.
 * - `remove`: delete the item at `index`.
 * - `update`: replace the item at `index`.
 * - `move`: take the item at `from` out of the list and put it back so that it
 *   ends up at index `to`.
 * - `clear`: empty the list.
 */
export type PatchOp<T> =
  | { readonly op: "insert"; readonly index: number; readonly item: T }
  | { readonly op: "remove"; readonly index: number }
  | { readonly op: "update"; readonly index: number; readonly item: T }
  | { readonly op: "move"; readonly from: number; readonly to: number }
  | { readonly op: "clear" };

const PATCH_INSERT = 0;
const PATCH_REMOVE = 1;
const PATCH_UPDATE = 2;
const PATCH_MOVE = 3;
const PATCH_CLEAR = 4;

/** Writes a keyed patch (a `u32` count followed by the operations) to `w`. */
export function encodePatch<T>(w: UndraWriter, ops: readonly PatchOp<T>[], item: Codec<T>): void {
  w.writeLen(ops.length);
  for (const op of ops) {
    switch (op.op) {
      case "insert":
        w.writeU8(PATCH_INSERT);
        w.writeU32(op.index);
        item.encode(w, op.item);
        break;
      case "remove":
        w.writeU8(PATCH_REMOVE);
        w.writeU32(op.index);
        break;
      case "update":
        w.writeU8(PATCH_UPDATE);
        w.writeU32(op.index);
        item.encode(w, op.item);
        break;
      case "move":
        w.writeU8(PATCH_MOVE);
        w.writeU32(op.from);
        w.writeU32(op.to);
        break;
      case "clear":
        w.writeU8(PATCH_CLEAR);
        break;
    }
  }
}

/**
 * Reads a keyed patch from `r`, decoding items with `item`. It does not check
 * for trailing bytes: when the patch is the whole `value` of a
 * {@link ChangeOp.KeyedPatch} entry, call `r.finish()` afterwards.
 */
export function decodePatch<T>(r: UndraReader, item: Codec<T>): PatchOp<T>[] {
  const count = r.readLen();
  const ops = new Array<PatchOp<T>>(count);
  for (let i = 0; i < count; i++) {
    const at = r.position;
    const tag = r.readU8();
    switch (tag) {
      case PATCH_INSERT: {
        const index = r.readU32();
        ops[i] = { op: "insert", index, item: item.decode(r) };
        break;
      }
      case PATCH_REMOVE:
        ops[i] = { op: "remove", index: r.readU32() };
        break;
      case PATCH_UPDATE: {
        const index = r.readU32();
        ops[i] = { op: "update", index, item: item.decode(r) };
        break;
      }
      case PATCH_MOVE: {
        const from = r.readU32();
        ops[i] = { op: "move", from, to: r.readU32() };
        break;
      }
      case PATCH_CLEAR:
        ops[i] = { op: "clear" };
        break;
      default:
        throw invalidTag(tag, at, "PatchOp");
    }
  }
  return ops;
}

function inBounds(index: number, limit: number): boolean {
  return Number.isInteger(index) && index >= 0 && index < limit;
}

/**
 * Applies a keyed patch to `list` and returns the result as a **new array**;
 * `list` is never modified and items are not cloned, so unchanged items keep
 * their identity (what React and other reference-equality consumers need).
 * The result is always a fresh array, even for an empty patch.
 *
 * Throws {@link PatchError} if an operation's index is out of bounds for the
 * list as it stands at that point, which means the mirror and the core have
 * diverged.
 */
export function applyPatch<T>(list: readonly T[], ops: readonly PatchOp<T>[]): T[] {
  const out = list.slice();
  for (let i = 0; i < ops.length; i++) {
    const op = ops[i] as PatchOp<T>;
    switch (op.op) {
      case "insert":
        if (!inBounds(op.index, out.length + 1)) throw new PatchError(i, op.op, op.index, out.length);
        out.splice(op.index, 0, op.item);
        break;
      case "remove":
        if (!inBounds(op.index, out.length)) throw new PatchError(i, op.op, op.index, out.length);
        out.splice(op.index, 1);
        break;
      case "update":
        if (!inBounds(op.index, out.length)) throw new PatchError(i, op.op, op.index, out.length);
        out[op.index] = op.item;
        break;
      case "move": {
        if (!inBounds(op.from, out.length)) throw new PatchError(i, op.op, op.from, out.length);
        if (!inBounds(op.to, out.length)) throw new PatchError(i, op.op, op.to, out.length);
        if (op.from !== op.to) {
          const moved = out.splice(op.from, 1)[0] as T;
          out.splice(op.to, 0, moved);
        }
        break;
      }
      case "clear":
        out.length = 0;
        break;
    }
  }
  return out;
}
