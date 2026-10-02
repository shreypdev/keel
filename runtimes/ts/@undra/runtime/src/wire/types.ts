import { WireError } from "./errors.js";

/** Raw bytes on the wire (`Bytes` in the schema). */
export type Bytes = Uint8Array;

// ---------------------------------------------------------------------------
// Handle (docs/SPEC.md section 1.2)
// ---------------------------------------------------------------------------

/**
 * Reference to an object owned by the core: a `u64` whose low 24 bits are the
 * slot index and whose high 40 bits are the generation (starting at 1; ADR-040
 * repartitioned ADR-022's 32/32). `0n` is the null handle. Handles are opaque
 * to hosts and only meaningful inside the runtime instance that issued them.
 */
export type Handle = bigint;

/** The null handle. */
export const NULL_HANDLE: Handle = 0n;

const MASK32 = 0xffff_ffffn;
/** The largest slot index of a handle (24 bits). */
export const MAX_HANDLE_INDEX = 0xff_ffff;
/** The largest generation of a handle (40 bits). */
export const MAX_HANDLE_GENERATION = 0xff_ffff_ffff;

/** The two 32-bit halves of a handle, as used by the wasm ABI (`handle_lo`, `handle_hi`). */
export interface HandleParts {
  /** Low 32 bits, as an unsigned number. */
  readonly lo: number;
  /** High 32 bits, as an unsigned number. */
  readonly hi: number;
}

/** Slot index of a handle (its low 24 bits). */
export function handleIndex(handle: Handle): number {
  return Number(handle & 0xff_ffffn);
}

/** Generation of a handle (its high 40 bits). */
export function handleGeneration(handle: Handle): number {
  return Number((handle >> 24n) & 0xff_ffff_ffffn);
}

/** Builds a handle from a slot index (24 bits) and a generation (40 bits). */
export function makeHandle(index: number, generation: number): Handle {
  if (!Number.isInteger(index) || index < 0 || index > MAX_HANDLE_INDEX) {
    throw new RangeError(`handle index out of range: ${String(index)}`);
  }
  if (!Number.isInteger(generation) || generation < 0 || generation > MAX_HANDLE_GENERATION) {
    throw new RangeError(`handle generation out of range: ${String(generation)}`);
  }
  return (BigInt(generation) << 24n) | BigInt(index);
}

/**
 * Splits a handle into unsigned 32-bit halves for the wasm ABI, which passes a
 * `u64` as two `i32` parameters so that hosts need no `BigInt` support in the
 * call path. JS converts an unsigned number to the wasm `i32` it wraps to, so
 * the halves can be passed to the exports as they are.
 */
export function splitHandle(handle: Handle): HandleParts {
  if (BigInt.asUintN(64, handle) !== handle) {
    throw new RangeError(`handle out of range: ${String(handle)}`);
  }
  return { lo: Number(handle & MASK32), hi: Number(handle >> 32n) };
}

/**
 * Joins the halves produced by {@link splitHandle}. Wasm hands `i32` values
 * back to JS as *signed* numbers; both halves are reinterpreted as unsigned
 * here, so `joinHandle(-1, -1)` is `0xffffffffffffffffn`.
 */
export function joinHandle(lo: number, hi: number): Handle {
  return (BigInt(hi >>> 0) << 32n) | BigInt(lo >>> 0);
}

// ---------------------------------------------------------------------------
// Timestamp (i64 milliseconds since the Unix epoch)
// ---------------------------------------------------------------------------

/** Milliseconds since the Unix epoch (the wire's `i64` milliseconds, as a JS `number`). */
export type Timestamp = number;

/** Converts a `Date` to a {@link Timestamp}. Throws `RangeError` for an invalid `Date`. */
export function timestampFromDate(date: Date): Timestamp {
  const ms = date.getTime();
  if (Number.isNaN(ms)) throw new RangeError("cannot convert an invalid Date to a Timestamp");
  return ms;
}

/**
 * Converts a {@link Timestamp} to a `Date`. Throws `RangeError` if the
 * timestamp is outside the range a `Date` can represent (about +-8.64e15 ms).
 */
export function timestampToDate(timestamp: Timestamp): Date {
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) {
    throw new RangeError(`timestamp ${String(timestamp)} is outside the range of a Date`);
  }
  return date;
}

// ---------------------------------------------------------------------------
// Duration (milliseconds in JS, i64 nanoseconds on the wire)
// ---------------------------------------------------------------------------

/**
 * A non-negative span of time in milliseconds (fractions allowed down to a
 * nanosecond). On the wire it is an `i64` count of nanoseconds; durations are
 * unsigned in every language, so a negative value is `negative_duration`.
 */
export type Duration = number;

const NANOS_PER_MS = 1_000_000n;
const I64_MAX = 0x7fff_ffff_ffff_ffffn;

/**
 * Converts a {@link Duration} to wire nanoseconds. Integer milliseconds are
 * converted exactly; fractions are rounded to the nearest nanosecond.
 *
 * Throws `WireError` (`negative_duration`) for a negative duration and
 * `RangeError` for a non-finite one or one beyond `i64` nanoseconds (about
 * 292 years).
 */
export function durationToNanos(ms: Duration): bigint {
  if (!Number.isFinite(ms)) throw new RangeError(`duration must be finite: ${String(ms)}`);
  const whole = Math.trunc(ms);
  let nanos = BigInt(whole) * NANOS_PER_MS;
  if (whole !== ms) nanos += BigInt(Math.round((ms - whole) * 1e6));
  if (nanos < 0n) throw new WireError({ code: "negative_duration", nanos });
  if (nanos > I64_MAX) throw new RangeError(`duration exceeds i64 nanoseconds: ${String(ms)} ms`);
  return nanos;
}

/**
 * Converts wire nanoseconds to a {@link Duration}. The whole-millisecond part
 * is exact; the fraction is as precise as a double allows. Throws `WireError`
 * (`negative_duration`) for a negative count.
 */
export function durationFromNanos(nanos: bigint): Duration {
  if (nanos < 0n) throw new WireError({ code: "negative_duration", nanos });
  return Number(nanos / NANOS_PER_MS) + Number(nanos % NANOS_PER_MS) / 1e6;
}

// ---------------------------------------------------------------------------
// Uuid (16 raw bytes, RFC 4122 order)
// ---------------------------------------------------------------------------

/** A UUID in canonical form: 36 characters, lowercase hex, hyphens after byte 4, 6, 8 and 10. */
export type Uuid = string;

const UUID_LEN = 36;
/** The two hex digits of every byte. */
const HEX: string[] = [];
for (let i = 256; i < 512; i++) HEX.push(i.toString(16).slice(1));

function hexValue(c: number): number {
  if (c >= 0x30 && c <= 0x39) return c - 0x30;
  const lower = c | 0x20;
  if (lower >= 0x61 && lower <= 0x66) return lower - 0x57;
  return NaN;
}

/**
 * Parses the canonical text form of a UUID (8-4-4-4-12 hex digits, either case) into its 16 bytes, written to `out` at
 * `offset` (a new array without `out`). Throws `RangeError` for anything else, or when the bytes do not fit.
 */
export function encodeUuid(uuid: Uuid, out?: Uint8Array, offset = 0): Uint8Array {
  const dst = out ?? new Uint8Array(16);
  let o = offset;
  let bad = uuid.length !== UUID_LEN || !Number.isInteger(offset) || offset < 0 || offset + 16 > dst.length;
  for (let i = 0; !bad && i < UUID_LEN; i += 2) {
    if (i === 8 || i === 13 || i === 18 || i === 23) bad = uuid.charCodeAt(i++) !== 0x2d;
    const byte = hexValue(uuid.charCodeAt(i)) * 16 + hexValue(uuid.charCodeAt(i + 1));
    dst[o++] = byte;
    bad ||= byte !== byte;
  }
  if (bad) throw new RangeError(`invalid UUID "${uuid}" (or 16 bytes do not fit at offset ${String(offset)} of ${dst.length})`);
  return dst;
}

/** The canonical lower-case text form of the 16 bytes of `bytes` at `offset`. Throws `RangeError` when there are fewer. */
export function decodeUuid(bytes: Uint8Array, offset = 0): Uuid {
  if (!Number.isInteger(offset) || offset < 0 || offset + 16 > bytes.length) {
    throw new RangeError(`need 16 bytes for a UUID at offset ${String(offset)} of ${bytes.length}`);
  }
  let out = "";
  for (let i = 0; i < 16; i++) out += (i === 4 || i === 6 || i === 8 || i === 10 ? "-" : "") + HEX[bytes[offset + i] as number];
  return out;
}
