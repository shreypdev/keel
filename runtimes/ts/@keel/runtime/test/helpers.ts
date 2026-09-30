import { expect } from "vitest";
import { WireError, type WireErrorCode, type WireErrorDetail } from "../src/wire/errors.js";

/** Lowercase hex of `bytes`. */
export function toHex(bytes: Uint8Array): string {
  let out = "";
  for (const b of bytes) out += b.toString(16).padStart(2, "0");
  return out;
}

/** Bytes of a hex string (whitespace ignored). */
export function fromHex(hex: string): Uint8Array {
  const clean = hex.replace(/\s+/g, "");
  if (clean.length % 2 !== 0) throw new Error(`odd-length hex: ${hex}`);
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = Number.parseInt(clean.slice(2 * i, 2 * i + 2), 16);
  return out;
}

/** Runs `fn` and returns the `WireError` it throws; fails the test on anything else. */
export function catchWireError(fn: () => unknown): WireError {
  try {
    fn();
  } catch (e) {
    if (e instanceof WireError) return e;
    throw e;
  }
  throw new Error("expected a WireError, but nothing was thrown");
}

/** Asserts `fn` throws a `WireError` with `code` and (a subset of) `fields`; returns the detail. */
export function expectWireError<C extends WireErrorCode>(
  fn: () => unknown,
  code: C,
  fields: Partial<Extract<WireErrorDetail, { code: C }>> = {},
): Extract<WireErrorDetail, { code: C }> {
  const err = catchWireError(fn);
  expect(err.code).toBe(code);
  expect(err.detail.code).toBe(code);
  expect(err.message.length).toBeGreaterThan(5);
  expect(err.detail).toMatchObject(fields);
  return err.detail as Extract<WireErrorDetail, { code: C }>;
}

/** Deterministic PRNG (mulberry32) so every failure reproduces from its seed. */
export function prng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** `n` pseudo-random bytes from `rand`. */
export function randomBytes(rand: () => number, n: number): Uint8Array {
  const out = new Uint8Array(n);
  for (let i = 0; i < n; i++) out[i] = Math.floor(rand() * 256);
  return out;
}

/** Pseudo-random integer in `[0, n)`. */
export function randInt(rand: () => number, n: number): number {
  return Math.floor(rand() * n);
}

/** Returns `bytes` placed inside a larger buffer with junk on both sides, as a view (non-zero `byteOffset`). */
export function embedded(bytes: Uint8Array, before = 5, after = 7): Uint8Array {
  const big = new Uint8Array(before + bytes.length + after).fill(0xaa);
  big.set(bytes, before);
  return big.subarray(before, before + bytes.length);
}
