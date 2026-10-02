import { describe, expect, it } from "vitest";
import { UndraReader } from "../src/wire/reader.js";
import { UndraWriter } from "../src/wire/writer.js";

/*
 * The byte-wise writer and reader of ADR-056 against a plain reference built from `DataView`, `TextEncoder` and
 * `TextDecoder` (what the wire module was before the call-path levers): random operation sequences, strings around the
 * 24-, 48- and 64-byte thresholds with non-ASCII units at every offset, 64-bit and floating-point values at their limits
 * (-0, NaN), views at every byte offset. The reviewer ran the same generator against `main`'s wire module for 12,000
 * payloads and 12,000 inputs: identical bytes, values and errors.
 */

function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
type R = () => number;
const int = (r: R, lo: number, hi: number): number => lo + Math.floor(r() * (hi - lo + 1));
const pick = <T>(r: R, xs: readonly T[]): T => xs[Math.floor(r() * xs.length)] as T;

const F64S = [0, -0, 1, -1, NaN, Infinity, -Infinity, 5e-324, Number.MAX_VALUE, 0.1, 1e308, 16777217];
const U64S = [0n, 1n, 0xffffffffn, 0x100000000n, 0x7fffffffffffffffn, 0x8000000000000000n, 0xffffffffffffffffn];
const I64S = [0n, -1n, -0x8000000000000000n, 0x7fffffffffffffffn, -0x100000000n];
const NONASCII = ["é", "€", "日", "\u{1F600}", "\u{10FFFF}", "\uD83D", "\uDE00", "﻿", "\u0000", "\u0080", "߿", "ࠀ", "￿"];

function randStr(r: R): string {
  const n = pick(r, [0, 1, 2, 23, 24, 25, 47, 48, 49, 62, 63, 64, 65, 66, 100, 200, int(r, 0, 300)]);
  const at = new Set<number>(Array.from({ length: int(r, 0, 3) }, () => int(r, 0, n)));
  let s = "";
  for (let i = 0; i <= n; i++) {
    if (at.has(i)) s += pick(r, NONASCII);
    if (i < n) s += "abcdefghijklmnopqrstuvwxyz0123456789 _-/."[int(r, 0, 40)];
  }
  return s;
}

type Op =
  | { t: "u8" | "i8" | "u16" | "i16" | "u32" | "i32"; v: number }
  | { t: "u64" | "i64"; v: bigint }
  | { t: "f32" | "f64"; v: number }
  | { t: "bool"; v: boolean }
  | { t: "str"; v: string };

function randOp(r: R): Op {
  switch (int(r, 0, 12)) {
    case 0: return { t: "u8", v: pick(r, [0, 127, 128, 255, int(r, 0, 255)]) };
    case 1: return { t: "i8", v: pick(r, [0, -1, 127, -128, int(r, -128, 127)]) };
    case 2: return { t: "u16", v: pick(r, [0, 255, 256, 65535, int(r, 0, 65535)]) };
    case 3: return { t: "i16", v: pick(r, [0, -1, 32767, -32768, int(r, -32768, 32767)]) };
    case 4: return { t: "u32", v: pick(r, [0, 0xffffffff, 0x80000000, int(r, 0, 0xffffffff)]) };
    case 5: return { t: "i32", v: pick(r, [0, -1, 2147483647, -2147483648, int(r, -2147483648, 2147483647)]) };
    case 6: return { t: "u64", v: pick(r, [...U64S, BigInt(int(r, 0, 2 ** 53 - 1))]) };
    case 7: return { t: "i64", v: pick(r, [...I64S, BigInt(int(r, -(2 ** 53), 2 ** 53))]) };
    case 8: return { t: "f32", v: pick(r, [...F64S, r() * 1e6]) };
    case 9: return { t: "f64", v: pick(r, [...F64S, r() * 1e300]) };
    case 10: return { t: "bool", v: r() < 0.5 };
    default: return { t: "str", v: randStr(r) };
  }
}

/** The reference encoding of one operation. */
function reference(op: Op): number[] {
  const dv = new DataView(new ArrayBuffer(8));
  switch (op.t) {
    case "u8": dv.setUint8(0, op.v); return [...new Uint8Array(dv.buffer, 0, 1)];
    case "i8": dv.setInt8(0, op.v); return [...new Uint8Array(dv.buffer, 0, 1)];
    case "u16": dv.setUint16(0, op.v, true); return [...new Uint8Array(dv.buffer, 0, 2)];
    case "i16": dv.setInt16(0, op.v, true); return [...new Uint8Array(dv.buffer, 0, 2)];
    case "u32": dv.setUint32(0, op.v, true); return [...new Uint8Array(dv.buffer, 0, 4)];
    case "i32": dv.setInt32(0, op.v, true); return [...new Uint8Array(dv.buffer, 0, 4)];
    case "u64": dv.setBigUint64(0, op.v, true); return [...new Uint8Array(dv.buffer)];
    case "i64": dv.setBigInt64(0, op.v, true); return [...new Uint8Array(dv.buffer)];
    case "f32": dv.setFloat32(0, op.v, true); return [...new Uint8Array(dv.buffer, 0, 4)];
    case "f64": dv.setFloat64(0, op.v, true); return [...new Uint8Array(dv.buffer)];
    case "bool": return [op.v ? 1 : 0];
    case "str": {
      const bytes = new TextEncoder().encode(op.v);
      dv.setUint32(0, bytes.length, true);
      return [...new Uint8Array(dv.buffer, 0, 4), ...bytes];
    }
  }
}

function write(w: UndraWriter, op: Op): void {
  switch (op.t) {
    case "u8": return w.writeU8(op.v);
    case "i8": return w.writeI8(op.v);
    case "u16": return w.writeU16(op.v);
    case "i16": return w.writeI16(op.v);
    case "u32": return w.writeU32(op.v);
    case "i32": return w.writeI32(op.v);
    case "u64": return w.writeU64(op.v);
    case "i64": return w.writeI64(op.v);
    case "f32": return w.writeF32(op.v);
    case "f64": return w.writeF64(op.v);
    case "bool": return w.writeBool(op.v);
    case "str": return w.writeStr(op.v);
  }
}

function read(r: UndraReader, op: Op): unknown {
  switch (op.t) {
    case "u8": return r.readU8();
    case "i8": return r.readI8();
    case "u16": return r.readU16();
    case "i16": return r.readI16();
    case "u32": return r.readU32();
    case "i32": return r.readI32();
    case "u64": return r.readU64();
    case "i64": return r.readI64();
    case "f32": return r.readF32();
    case "f64": return r.readF64();
    case "bool": return r.readBool();
    case "str": return r.readStr();
  }
}

/** What the value reads back as: `f32` rounds, a lone surrogate becomes U+FFFD and a leading U+FEFF is kept, the rest is itself. */
function expected(op: Op): unknown {
  if (op.t === "f32") return Math.fround(op.v);
  if (op.t === "str") return new TextDecoder("utf-8", { ignoreBOM: true }).decode(new TextEncoder().encode(op.v));
  return op.v;
}

describe("the writer and the reader against a DataView and TextEncoder reference", () => {
  it("writes the reference's bytes and reads its values back, for 6,000 random sequences at any capacity and byte offset", () => {
    const r = rng(0x5eed);
    for (let n = 0; n < 6000; n++) {
      const ops = Array.from({ length: int(r, 1, 10) }, () => randOp(r));
      const w = new UndraWriter(pick(r, [0, 1, 16, 63, 64, 65, 256]));
      for (const op of ops) write(w, op);
      const bytes = w.finish();
      expect([...bytes], JSON.stringify(ops, (_k, v) => (typeof v === "bigint" ? `${v}n` : v))).toEqual(ops.flatMap(reference));
      // The same bytes in a view at an offset, as a change-set or a reply is read out of a larger buffer.
      const off = int(r, 0, 7);
      const host = new Uint8Array(bytes.length + off + 2);
      host.set(bytes, off);
      const rd = new UndraReader(host.subarray(off, off + bytes.length));
      for (const op of ops) expect(Object.is(read(rd, op), expected(op)), `${op.t} ${String(op.v)}`).toBe(true);
      rd.finish();
    }
  });

  it("reads the reference's strings, or refuses what TextDecoder refuses, for 6,000 random byte strings", () => {
    const r = rng(0xfeed);
    const strict = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
    for (let n = 0; n < 6000; n++) {
      const len = int(r, 0, 70);
      const body = Uint8Array.from({ length: len }, () => (r() < 0.6 ? int(r, 0x20, 0x7e) : pick(r, [0x80, 0xc2, 0xe2, 0xed, 0xf0, 0xf4, 0xff, int(r, 0, 255)])));
      const input = Uint8Array.from([len & 0xff, (len >> 8) & 0xff, 0, 0, ...body]);
      let want: string | undefined;
      try {
        want = strict.decode(body);
      } catch {
        want = undefined;
      }
      let got: string | undefined;
      try {
        got = new UndraReader(input).readStr();
      } catch {
        got = undefined;
      }
      expect(got).toBe(want);
    }
  });

  it("the one shared scratch is never held across code that can run another reader or writer", () => {
    // A codec that reads a value, encodes and decodes something else (a call from a listener, say), and reads on.
    const w = new UndraWriter();
    w.writeF64(1.5);
    w.writeU64(7n);
    w.writeF64(-2.25);
    const r = new UndraReader(w.finish());
    const first = r.readF64();
    const other = new UndraWriter();
    other.writeF64(99.5);
    other.writeU64(0xffff_ffff_ffff_ffffn);
    new UndraReader(other.finish()).readF64();
    expect([first, r.readU64(), r.readF64()]).toEqual([1.5, 7n, -2.25]);
    // A value whose coercion runs a writer of its own: the outer value is written whole.
    const reentrant = {
      valueOf(): number {
        const inner = new UndraWriter();
        inner.writeF64(123.456);
        inner.writeU64(5n);
        return 2.5;
      },
    } as unknown as number;
    const outer = new UndraWriter();
    outer.writeF64(reentrant);
    outer.writeF64(7);
    const back = new UndraReader(outer.finish());
    expect([back.readF64(), back.readF64()]).toEqual([2.5, 7]);
  });
});
