import type { Codec } from "./codec.js";
import { WireError } from "./errors.js";
import {
  CHANGE_ENTRY_MIN,
  CallTarget,
  type ChangeEntry,
  type ChangeSetPayload,
  PATCH_CLEAR,
  PATCH_INSERT,
  PATCH_MOVE,
  PATCH_REMOVE,
  PATCH_UPDATE,
  type PatchOp,
  type PortReplyPayload,
  PortStatus,
  ReplyStatus,
  decodeAll,
  decodeWithTail,
  invalidTag,
  readChangeEntry,
} from "./payloads.js";
import { UndraReader } from "./reader.js";
import { StreamFlag, type StreamFailure, readStreamFailure, writeStreamFailure } from "./stream-payloads.js";
import type { Handle } from "./types.js";
import { UndraWriter } from "./writer.js";

/*
 * The payloads only a framed transport (`remote`, the worker's) or a test encodes and decodes (docs/SPEC.md sections 3.3 to
 * 3.8 and 5.9): a core that runs in this thread passes its control messages as calls and never frames them (ADR-057), so a
 * `wasm-main` page does not carry this module. The payloads the page reads itself (the change-set, the port reply, the keyed
 * patch) are in `payloads.ts`; the stream failure in `stream-payloads.ts`; the lazy list pages in `lazy-payloads.ts`.
 */


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
  /** The store's type; listed in {@link SnapshotPayload.types}. */
  readonly typeId: number;
  /** Stored (non-computed) signals. */
  readonly signals: readonly SnapshotSignal[];
}

/** One store type of a snapshot and the fingerprint of its signals when it was taken (ADR-037). */
export interface SnapshotType {
  /** `fnv1a32("<TypeName>")` of the store type. */
  readonly typeId: number;
  /** `fnv1a64` of the canonical closure of the type's non-computed signals; restore compares it with the current build's. */
  readonly fingerprint: bigint;
}

/**
 * Payload of a `Snapshot` (core to host) and of a `Restore` (host to core): layout 2 of SPEC 5.9
 * (ADR-037), all little-endian:
 *
 * ```text
 * count u32, generation_floor u64, schema_hash u64,
 * type_count u32, types x { type_id u32, fingerprint u64 },
 * description_len u32, description (UTF-8 JSON, opaque to hosts),
 * count x { handle u64, type_id u32, signal_count u32, signals x { signal_id u32, len u32, value } }
 * ```
 *
 * A host treats a snapshot as opaque bytes (`UndraCore.snapshot`, `restore`); this codec exists for
 * tools and tests.
 */
export interface SnapshotPayload {
  /**
   * The highest handle generation the core had issued when the snapshot was taken. A restore
   * resumes the core's generation counter above it, so no handle issued before the snapshot (or
   * between it and the restore) is ever issued again to another object (ADR-022). A `u64` on the
   * wire (ADR-040: generations are 40 bits), read as a `number`, which holds every generation
   * exactly. Opaque to the host: pass it back unchanged.
   */
  readonly generationFloor: number;
  /** The schema hash of the core that took the snapshot. */
  readonly schemaHash: bigint;
  /** Each store type of the snapshot, once, with its fingerprint. */
  readonly types: readonly SnapshotType[];
  /** The canonical JSON description of the store types' closures (ADR-037), opaque to hosts. */
  readonly description: string;
  /**
   * Every record in the snapshot: a store's, and the recreation record of each query handle (ADR-059), which has one
   * signal whose id is `0xFFFF_FFFE` and whose value says what the handle is made of. Hosts need not tell them apart.
   */
  readonly stores: readonly SnapshotStore[];
}

/** Smallest encoded store: handle 8, type id 4, signal count 4. */
const SNAPSHOT_STORE_MIN = 16;
/** Smallest encoded signal: signal id 4, len 4. */
const SNAPSHOT_SIGNAL_MIN = 8;
/** An encoded type entry: type id 4, fingerprint 8. */
const SNAPSHOT_TYPE_LEN = 12;

/** Encodes a `Snapshot` or `Restore` payload (layout 2). */
export function encodeSnapshot(snapshot: SnapshotPayload): Uint8Array {
  const w = new UndraWriter();
  w.writeLen(snapshot.stores.length);
  w.writeU64Number(snapshot.generationFloor);
  w.writeU64(snapshot.schemaHash);
  w.writeLen(snapshot.types.length);
  for (const type of snapshot.types) {
    w.writeU32(type.typeId);
    w.writeU64(type.fingerprint);
  }
  w.writeStr(snapshot.description);
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

/**
 * Decodes a `Snapshot` or `Restore` payload (layout 2); signal values are borrowed views into `bytes`.
 * Throws {@link WireError} `duplicate_key` for a type listed twice, `invalid_tag` for a store whose type
 * is not listed, `invalid_utf8` for a description that is not UTF-8; a snapshot in the layout before
 * ADR-037 does not decode.
 */
export function decodeSnapshot(bytes: Uint8Array): SnapshotPayload {
  return decodeAll(bytes, (r) => {
    const storeCount = r.readLen(SNAPSHOT_STORE_MIN);
    const generationFloor = r.readU64Number();
    const schemaHash = r.readU64();
    const typeCount = r.readLen(SNAPSHOT_TYPE_LEN);
    const types = new Array<SnapshotType>(typeCount);
    const listed = new Set<number>();
    for (let i = 0; i < typeCount; i++) {
      const at = r.position;
      const typeId = r.readU32();
      const fingerprint = r.readU64();
      if (listed.has(typeId)) throw new WireError({ code: "duplicate_key", at });
      listed.add(typeId);
      types[i] = { typeId, fingerprint };
    }
    const description = r.readStr();
    const stores = new Array<SnapshotStore>(storeCount);
    for (let i = 0; i < storeCount; i++) {
      const at = r.position;
      const handle = r.readU64();
      const typeId = r.readU32();
      const signalCount = r.readLen(SNAPSHOT_SIGNAL_MIN);
      const signals = new Array<SnapshotSignal>(signalCount);
      for (let j = 0; j < signalCount; j++) {
        const signalId = r.readU32();
        signals[j] = { signalId, value: r.readBytes() };
      }
      if (!listed.has(typeId)) {
        throw new WireError({ code: "invalid_tag", tag: typeId, at, ty: "Snapshot store type (not in the type table)" });
      }
      stores[i] = { handle, typeId, signals };
    }
    return { generationFloor, schemaHash, types, description, stores };
  });
}

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
