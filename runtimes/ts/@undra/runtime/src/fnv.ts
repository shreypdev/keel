/*
 * FNV-1a over the UTF-8 bytes of a string (docs/SPEC.md section 1.1). The
 * Rust macros compute every type, method, port and query id with these
 * functions at compile time; the TypeScript runtime needs the same functions
 * to verify ids and to hash the schema.
 */

const ENCODER = new TextEncoder();

/** Scratch space for strings up to `SCRATCH.length / 3` UTF-16 units (UTF-8 needs at most 3 bytes per unit). */
const SCRATCH = new Uint8Array(3072);

const FNV32_OFFSET = 0x811c9dc5;
const FNV32_PRIME = 0x01000193;

/** FNV-1a 64-bit offset basis, split into 32-bit halves. */
const FNV64_OFFSET_HI = 0xcbf29ce4;
const FNV64_OFFSET_LO = 0x84222325;
/** The 64-bit prime 0x100000001b3 is 2^40 + 0x1b3. */
const FNV64_PRIME_LOW_PART = 0x1b3;

const TWO_POW_32 = 0x1_0000_0000;

/**
 * 32-bit FNV-1a of the UTF-8 encoding of `s`, as an unsigned integer.
 * A lone UTF-16 surrogate hashes as U+FFFD, as `TextEncoder` encodes it.
 *
 * `fnv1a32("Calculator.add") === 2353348832`.
 */
export function fnv1a32(s: string): number {
  let bytes = SCRATCH;
  let n: number;
  if (s.length * 3 <= SCRATCH.length) {
    n = ENCODER.encodeInto(s, SCRATCH).written;
  } else {
    bytes = ENCODER.encode(s);
    n = bytes.length;
  }
  let h = FNV32_OFFSET;
  for (let i = 0; i < n; i++) {
    h = Math.imul(h ^ (bytes[i] as number), FNV32_PRIME);
  }
  return h >>> 0;
}

/**
 * 64-bit FNV-1a of the UTF-8 encoding of `s`, as a `bigint`. The arithmetic
 * runs on two 32-bit halves, so hashing does not allocate a `BigInt` per byte.
 *
 * `fnv1a64("undra") === 6367360722358687308n`.
 */
export function fnv1a64(s: string): bigint {
  let bytes = SCRATCH;
  let n: number;
  if (s.length * 3 <= SCRATCH.length) {
    n = ENCODER.encodeInto(s, SCRATCH).written;
  } else {
    bytes = ENCODER.encode(s);
    n = bytes.length;
  }
  let hi = FNV64_OFFSET_HI;
  let lo = FNV64_OFFSET_LO;
  for (let i = 0; i < n; i++) {
    lo = (lo ^ (bytes[i] as number)) >>> 0;
    // (hi:lo) * (2^40 + 0x1b3) mod 2^64. Every intermediate stays below 2^53.
    const low = lo * FNV64_PRIME_LOW_PART;
    const carry = Math.floor(low / TWO_POW_32);
    // (hi:lo) << 40 keeps only the low 24 bits of lo, landing in bits 8..31 of the high half.
    const shifted = (lo << 8) >>> 0;
    hi = (hi * FNV64_PRIME_LOW_PART + carry + shifted) >>> 0;
    lo = low >>> 0;
  }
  return (BigInt(hi) << 32n) | BigInt(lo);
}
