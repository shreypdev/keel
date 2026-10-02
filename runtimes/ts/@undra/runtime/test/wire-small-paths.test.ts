import { describe, expect, it } from "vitest";
import { UndraReader, UndraWriter, WireError } from "../src/wire/index.js";

/*
 * The writer and the reader keep small buffers on V8's heap and read and write integers byte by byte (ADR-056): the
 * paths that differ from what a `DataView` over each buffer did, pinned against `DataView`, `TextEncoder` and
 * `TextDecoder`, which are the reference.
 */

const utf8 = new TextEncoder();
const lengthPrefixed = (text: string): Uint8Array => {
  const body = utf8.encode(text);
  const out = new Uint8Array(4 + body.length);
  new DataView(out.buffer).setUint32(0, body.length, true);
  out.set(body, 4);
  return out;
};

describe("UndraWriter", () => {
  it("keeps a result of at most 64 bytes in a buffer of exactly its length, and does not copy a larger one", () => {
    const small = new UndraWriter();
    small.writeU32(7);
    small.writeStr("hello");
    const out = small.finish();
    expect(out.length).toBe(13);
    expect(out.buffer.byteLength).toBe(13);

    const edge = new UndraWriter();
    edge.writeRaw(new Uint8Array(64).fill(1));
    expect(edge.finish().buffer.byteLength).toBe(64);

    const large = new UndraWriter();
    large.writeRaw(new Uint8Array(65).fill(2));
    const big = large.finish();
    expect(big.length).toBe(65);
    expect(big.buffer.byteLength).toBeGreaterThanOrEqual(65);
  });

  it("writes every integer width as a DataView would, at the edges, from any position", () => {
    const w = new UndraWriter(0);
    const reference = new DataView(new ArrayBuffer(128));
    let at = 0;
    const both = (write: (w: UndraWriter) => void, set: (v: DataView, at: number) => number): void => {
      write(w);
      at += set(reference, at);
    };
    both((x) => x.writeU8(255), (v, a) => (v.setUint8(a, 255), 1));
    both((x) => x.writeI8(-128), (v, a) => (v.setInt8(a, -128), 1));
    both((x) => x.writeI8(127), (v, a) => (v.setInt8(a, 127), 1));
    both((x) => x.writeU16(0xffff), (v, a) => (v.setUint16(a, 0xffff, true), 2));
    both((x) => x.writeI16(-32768), (v, a) => (v.setInt16(a, -32768, true), 2));
    both((x) => x.writeI16(32767), (v, a) => (v.setInt16(a, 32767, true), 2));
    both((x) => x.writeU32(0xffff_ffff), (v, a) => (v.setUint32(a, 0xffff_ffff, true), 4));
    both((x) => x.writeU32(0x8000_0001), (v, a) => (v.setUint32(a, 0x8000_0001, true), 4));
    both((x) => x.writeI32(-2147483648), (v, a) => (v.setInt32(a, -2147483648, true), 4));
    both((x) => x.writeI32(-1), (v, a) => (v.setInt32(a, -1, true), 4));
    both((x) => x.writeLen(0x1234_5678), (v, a) => (v.setUint32(a, 0x1234_5678, true), 4));
    both((x) => x.writeU64Number(Number.MAX_SAFE_INTEGER), (v, a) => (v.setBigUint64(a, BigInt(Number.MAX_SAFE_INTEGER), true), 8));
    both((x) => x.writeI64Number(-123456789012345), (v, a) => (v.setBigInt64(a, -123456789012345n, true), 8));
    both((x) => x.writeI64Number(Number.MIN_SAFE_INTEGER), (v, a) => (v.setBigInt64(a, BigInt(Number.MIN_SAFE_INTEGER), true), 8));
    both((x) => x.writeU64(0xfedc_ba98_7654_3210n), (v, a) => (v.setBigUint64(a, 0xfedc_ba98_7654_3210n, true), 8));
    both((x) => x.writeI64(-0x7edc_ba98_7654_3210n), (v, a) => (v.setBigInt64(a, -0x7edc_ba98_7654_3210n, true), 8));
    both((x) => x.writeF32(1.5), (v, a) => (v.setFloat32(a, 1.5, true), 4));
    both((x) => x.writeF64(-2.25e100), (v, a) => (v.setFloat64(a, -2.25e100, true), 8));
    expect(w.finish()).toEqual(new Uint8Array(reference.buffer, 0, at));
  });

  it("shares one scratch between writers without mixing their 64-bit values", () => {
    const a = new UndraWriter();
    const b = new UndraWriter();
    a.writeU64(0x0102_0304_0506_0708n);
    b.writeU64(0x1112_1314_1516_1718n);
    a.writeF64(1);
    b.writeF32(2);
    expect(a.finish()).toEqual(Uint8Array.of(8, 7, 6, 5, 4, 3, 2, 1, 0, 0, 0, 0, 0, 0, 0xf0, 0x3f));
    expect(b.finish()).toEqual(Uint8Array.of(0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11, 0, 0, 0, 0x40));
  });

  it("writes a string that is ASCII for a while and then is not, whole, wherever the first other unit falls", () => {
    for (const capacity of [0, 8, 64]) {
      for (let length = 1; length <= 48; length++) {
        for (let at = 0; at < length; at++) {
          for (const other of ["é", "€", "😀"]) {
            if (other === "😀" && at + 1 >= length) continue; // a surrogate pair takes two units
            const units = Array.from({ length }, () => "x");
            units[at] = other;
            const text = units.join("").slice(0, other === "😀" ? length + 1 : length);
            const w = new UndraWriter(capacity);
            w.writeStr(text);
            w.writeU8(0x5a);
            expect(w.finish(), `${JSON.stringify(text)} at ${at}, capacity ${capacity}`).toEqual(
              Uint8Array.of(...lengthPrefixed(text), 0x5a),
            );
          }
        }
      }
    }
  });

  it("writes a lone surrogate as U+FFFD, as TextEncoder does, in the short and the long path", () => {
    for (const text of ["a\ud800b", "\udc00", "x".repeat(60) + "\ud800" + "y"]) {
      const w = new UndraWriter();
      w.writeStr(text);
      expect(w.finish()).toEqual(lengthPrefixed(text));
    }
  });
});

describe("UndraReader", () => {
  it("reads every integer width as a DataView does, at any offset of a view into a larger buffer", () => {
    const backing = new Uint8Array(96).map((_, i) => (i * 37 + 11) & 0xff);
    const view = new DataView(backing.buffer);
    for (const offset of [0, 1, 3, 7, 13]) {
      const bytes = backing.subarray(offset, offset + 60);
      const reference = new DataView(backing.buffer, offset, 60);
      for (let at = 0; at <= 15; at += 5) {
        const r = new UndraReader(bytes.subarray(at));
        expect(r.readU8()).toBe(reference.getUint8(at));
        expect(r.readI8()).toBe(reference.getInt8(at + 1));
        expect(r.readU16()).toBe(reference.getUint16(at + 2, true));
        expect(r.readI16()).toBe(reference.getInt16(at + 4, true));
        expect(r.readU32()).toBe(reference.getUint32(at + 6, true));
        expect(r.readI32()).toBe(reference.getInt32(at + 10, true));
        expect(r.readU64()).toBe(reference.getBigUint64(at + 14, true));
        expect(r.readI64()).toBe(reference.getBigInt64(at + 22, true));
        expect(r.readF32()).toBe(reference.getFloat32(at + 30, true));
        expect(r.readF64()).toBe(reference.getFloat64(at + 34, true));
        expect(r.position).toBe(42);
      }
    }
    expect(view.byteLength).toBe(96);
  });

  it("reads two inputs' 64-bit values alternately through the one shared scratch", () => {
    const a = new UndraReader(Uint8Array.of(1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xf0, 0x3f));
    const b = new UndraReader(Uint8Array.of(2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x20, 0x40));
    expect(a.readU64()).toBe(1n);
    expect(b.readU64()).toBe(2n);
    expect(a.readF32()).toBe(1.875);
    expect(b.readF32()).toBe(2.5);
  });

  it("reads a short ASCII string by the loop and anything else by TextDecoder, with the same result and errors", () => {
    for (let length = 0; length <= 30; length++) {
      const text = "abcdefghijklmnopqrstuvwxyz0123456789".slice(0, length);
      expect(new UndraReader(lengthPrefixed(text)).readStr()).toBe(text);
    }
    for (const text of ["é", "naïve", "x".repeat(23) + "é", "€uro", "😀 ok", "﻿bom", "x".repeat(40)]) {
      const r = new UndraReader(lengthPrefixed(text));
      expect(r.readStr()).toBe(text);
      expect(r.remaining).toBe(0);
    }
    for (const body of [[0x61, 0xff], [0xc3], [0x80], [0x61, 0x62, 0xe2, 0x82]]) {
      const bytes = Uint8Array.of(body.length, 0, 0, 0, ...body);
      let caught: unknown;
      try {
        new UndraReader(bytes).readStr();
      } catch (error) {
        caught = error;
      }
      expect(caught, JSON.stringify(body)).toBeInstanceOf(WireError);
      expect((caught as WireError).code).toBe("invalid_utf8");
    }
  });

  it("does not move past what it failed to read", () => {
    const r = new UndraReader(Uint8Array.of(1, 2, 3));
    expect(r.readU16()).toBe(0x0201);
    expect(() => r.readU32()).toThrow(WireError);
    expect(r.position).toBe(2);
    expect(r.readU8()).toBe(3);
  });
});
