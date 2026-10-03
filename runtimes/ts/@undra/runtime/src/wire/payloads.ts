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

export function invalidTag(tag: number, at: number, ty: string): WireError {
  return new WireError({ code: "invalid_tag", tag, at, ty });
}

/** Decodes a whole payload with `read` and asserts nothing is left over. */
export function decodeAll<T>(bytes: Uint8Array, read: (r: UndraReader) => T): T {
  const r = new UndraReader(bytes);
  const value = read(r);
  r.finish();
  return value;
}

/** Decodes a payload whose last field is an opaque tail consumed by `read` via `readRest`. */
export function decodeWithTail<T>(bytes: Uint8Array, read: (r: UndraReader) => T): T {
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
export const CHANGE_ENTRY_MIN = 17;

export function readChangeEntry(r: UndraReader): ChangeEntry {
  const handle = r.readU64();
  const signalId = r.readU32();
  const at = r.position;
  const op = r.readU8();
  if (op > ChangeOp.LazyInvalidated) throw invalidTag(op, at, "ChangeOp");
  return { handle, signalId, op: op as ChangeOp, value: r.readBytes() };
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

export const PATCH_INSERT = 0;
export const PATCH_REMOVE = 1;
export const PATCH_UPDATE = 2;
export const PATCH_MOVE = 3;
export const PATCH_CLEAR = 4;

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
