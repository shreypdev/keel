import { WireError } from "./errors.js";
import { UndraReader } from "./reader.js";

/**
 * Message kind carried in an envelope header (docs/SPEC.md section 3.2). The
 * numeric values are the wire values.
 */
export enum Kind {
  /** host to core: invoke a function, method, constructor or lazy-list page. */
  Call = 1,
  /** core to host: outcome of a call. */
  Reply = 2,
  /** core to host: signal updates of one transaction. */
  ChangeSet = 3,
  /** core to host: invoke a platform port. */
  PortCall = 4,
  /** host to core: outcome of a port call. */
  PortReply = 5,
  /** host to core: cancel an in-flight call or stream. */
  Cancel = 6,
  /** host to core: grant a stream more credit. */
  StreamCredit = 7,
  /** core to host: one stream item, the end, or an error. */
  StreamItem = 8,
  /** host to core: start or stop observing a signal. */
  Observe = 9,
  /** host to core: release an object handle. */
  Release = 10,
  /** host to core: deliver a port event. */
  Event = 11,
  /** both ways: version and schema handshake. */
  Hello = 12,
  /** core to host: a log record. */
  Log = 13,
  /** host to core: a timer set through the Timer port is due. */
  TimerFired = 14,
  /** core to host: a snapshot of every store. */
  Snapshot = 15,
  /** host to core: restore from a snapshot. */
  Restore = 16,
}

const MIN_KIND = Kind.Call;
const MAX_KIND = Kind.Restore;

/** Size in bytes of the envelope header: magic 4, version 2, schema 8, kind 1, seq 4, len 4. */
export const HEADER_LEN = 23;

/** The wire version this runtime speaks. */
export const WIRE_VERSION = 1;

/** The magic bytes `4B 45 45 4C` read as a little-endian `u32`. */
const MAGIC = 0x4c45454b;
const U32_MAX = 0xffff_ffff;

/** A decoded envelope. `payload` is a borrowed view into the decoded buffer. */
export interface Envelope {
  /** What the payload is. */
  readonly kind: Kind;
  /** Per-direction message counter, for ordering and debugging. */
  readonly seq: number;
  /** Schema hash of the core that produced or expects the message. */
  readonly schemaHash: bigint;
  /** The payload bytes; a view into the input, not a copy (see `UndraReader.readBytes`). */
  readonly payload: Uint8Array;
}

/**
 * Frames `payload` as an envelope: the 23-byte header followed by the payload,
 * in one allocation.
 *
 * Throws `RangeError` if `kind` is not a {@link Kind}, or `seq` / `schemaHash`
 * do not fit their `u32` / `u64` fields.
 */
export function encodeEnvelope(kind: Kind, seq: number, schemaHash: bigint, payload: Uint8Array): Uint8Array {
  if (!Number.isInteger(kind) || kind < MIN_KIND || kind > MAX_KIND) {
    throw new RangeError(`unknown envelope kind: ${String(kind)}`);
  }
  if (seq >>> 0 !== seq) throw new RangeError(`envelope seq out of range: ${String(seq)}`);
  if (BigInt.asUintN(64, schemaHash) !== schemaHash) {
    throw new RangeError(`envelope schema hash out of range: ${String(schemaHash)}`);
  }
  if (payload.length > U32_MAX) {
    throw new WireError({ code: "length_too_large", len: payload.length, at: HEADER_LEN - 4 });
  }
  const out = new Uint8Array(HEADER_LEN + payload.length);
  const view = new DataView(out.buffer);
  view.setUint32(0, MAGIC, true);
  view.setUint16(4, WIRE_VERSION, true);
  view.setBigUint64(6, schemaHash, true);
  view.setUint8(14, kind);
  view.setUint32(15, seq, true);
  view.setUint32(19, payload.length, true);
  out.set(payload, HEADER_LEN);
  return out;
}

/**
 * Parses an envelope, validating the magic, the version, the kind and that the
 * declared payload length matches the bytes present exactly. The payload is a
 * view into `bytes`.
 *
 * The schema hash is returned, not judged, because a `Hello` must be readable
 * across a mismatch to report it. Pass `expectedSchema` for every other
 * message to have a different hash fail with `schema_mismatch`.
 *
 * Errors: `unexpected_eof` (truncated header), `bad_magic`,
 * `unsupported_version`, `invalid_tag` (unknown kind), `length_too_large`
 * (the payload is shorter than declared), `trailing_bytes` (it is longer),
 * `schema_mismatch`.
 */
export function decodeEnvelope(bytes: Uint8Array, expectedSchema?: bigint): Envelope {
  const r = new UndraReader(bytes);
  if (r.readU32() !== MAGIC) throw new WireError({ code: "bad_magic" });
  const version = r.readU16();
  if (version !== WIRE_VERSION) throw new WireError({ code: "unsupported_version", version });
  const schemaHash = r.readU64();
  const kindAt = r.position;
  const kind = r.readU8();
  if (kind < MIN_KIND || kind > MAX_KIND) {
    throw new WireError({ code: "invalid_tag", tag: kind, at: kindAt, ty: "Kind" });
  }
  const seq = r.readU32();
  const lenAt = r.position;
  const len = r.readU32();
  if (len > r.remaining) throw new WireError({ code: "length_too_large", len, at: lenAt });
  const payload = r.readRaw(len);
  r.finish();
  if (expectedSchema !== undefined && schemaHash !== expectedSchema) {
    throw new WireError({ code: "schema_mismatch", expected: expectedSchema, got: schemaHash });
  }
  return { kind: kind as Kind, seq, schemaHash, payload };
}
