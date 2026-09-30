import { describe, expect, it } from "vitest";
import { KeelWriter } from "../src/wire/writer.js";
import { expectWireError, fromHex, prng, randInt, toHex } from "./helpers.js";

function written(fn: (w: KeelWriter) => void): string {
  const w = new KeelWriter();
  fn(w);
  return toHex(w.finish());
}

describe("KeelWriter primitives", () => {
  const table: [string, (w: KeelWriter) => void, string][] = [
    ["u8 0", (w) => w.writeU8(0), "00"],
    ["u8 255", (w) => w.writeU8(255), "ff"],
    ["i8 -128", (w) => w.writeI8(-128), "80"],
    ["i8 127", (w) => w.writeI8(127), "7f"],
    ["i8 -1", (w) => w.writeI8(-1), "ff"],
    ["u16 0x1234", (w) => w.writeU16(0x1234), "3412"],
    ["u16 max", (w) => w.writeU16(0xffff), "ffff"],
    ["i16 min", (w) => w.writeI16(-32768), "0080"],
    ["i16 max", (w) => w.writeI16(32767), "ff7f"],
    ["u32 0x12345678", (w) => w.writeU32(0x12345678), "78563412"],
    ["u32 max", (w) => w.writeU32(0xffffffff), "ffffffff"],
    ["i32 min", (w) => w.writeI32(-2147483648), "00000080"],
    ["i32 max", (w) => w.writeI32(2147483647), "ffffff7f"],
    ["i32 -2", (w) => w.writeI32(-2), "feffffff"],
    ["u64 max", (w) => w.writeU64(2n ** 64n - 1n), "ffffffffffffffff"],
    ["u64 0x0102030405060708", (w) => w.writeU64(0x0102030405060708n), "0807060504030201"],
    ["i64 min", (w) => w.writeI64(-(2n ** 63n)), "0000000000000080"],
    ["i64 max", (w) => w.writeI64(2n ** 63n - 1n), "ffffffffffffff7f"],
    ["i64 -(2^53+1)", (w) => w.writeI64(-9007199254740993n), "ffffffffffffdfff"],
    ["f32 3.14", (w) => w.writeF32(3.14), "c3f54840"],
    ["f64 e", (w) => w.writeF64(Math.E), "6957148b0abf0540"],
    ["f64 -0", (w) => w.writeF64(-0), "0000000000000080"],
    ["f64 +inf", (w) => w.writeF64(Number.POSITIVE_INFINITY), "000000000000f07f"],
    ["bool true", (w) => w.writeBool(true), "01"],
    ["bool false", (w) => w.writeBool(false), "00"],
    ["len 3", (w) => w.writeLen(3), "03000000"],
    ["len max", (w) => w.writeLen(0xffffffff), "ffffffff"],
    ["str empty", (w) => w.writeStr(""), "00000000"],
    ["str héllo", (w) => w.writeStr("héllo \u{1f30a}"), "0b00000068c3a96c6c6f20f09f8c8a"],
    ["bytes", (w) => w.writeBytes(new Uint8Array([1, 2, 3, 255])), "04000000010203ff"],
    ["bytes empty", (w) => w.writeBytes(new Uint8Array(0)), "00000000"],
    ["raw", (w) => w.writeRaw(new Uint8Array([9, 8])), "0908"],
    ["uuid", (w) => w.writeUuid("123e4567-e89b-12d3-a456-426614174000"), "123e4567e89b12d3a456426614174000"],
  ];
  it.each(table)("%s", (_name, fn, hex) => {
    expect(written(fn)).toBe(hex);
  });

  it("writes NaN as a NaN of the right width", () => {
    const w = new KeelWriter();
    w.writeF32(Number.NaN);
    w.writeF64(Number.NaN);
    const bytes = w.finish();
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    expect(Number.isNaN(view.getFloat32(0, true))).toBe(true);
    expect(Number.isNaN(view.getFloat64(4, true))).toBe(true);
  });

  it("rounds f32 to single precision", () => {
    const w = new KeelWriter();
    w.writeF32(16777217); // 2^24 + 1 is not representable
    const bytes = w.finish();
    expect(new DataView(bytes.buffer, bytes.byteOffset, 4).getFloat32(0, true)).toBe(16777216);
  });

  it("sequences writes without gaps or padding", () => {
    expect(
      written((w) => {
        w.writeU8(1);
        w.writeU16(2);
        w.writeU32(3);
        w.writeU64(4n);
      }),
    ).toBe("01" + "0200" + "03000000" + "0400000000000000");
  });
});

describe("KeelWriter range validation", () => {
  const rejects: [string, (w: KeelWriter) => void][] = [
    ["u8 256", (w) => w.writeU8(256)],
    ["u8 -1", (w) => w.writeU8(-1)],
    ["u8 1.5", (w) => w.writeU8(1.5)],
    ["u8 NaN", (w) => w.writeU8(Number.NaN)],
    ["i8 128", (w) => w.writeI8(128)],
    ["i8 -129", (w) => w.writeI8(-129)],
    ["u16 65536", (w) => w.writeU16(65536)],
    ["u16 -1", (w) => w.writeU16(-1)],
    ["i16 32768", (w) => w.writeI16(32768)],
    ["i16 -32769", (w) => w.writeI16(-32769)],
    ["u32 2^32", (w) => w.writeU32(2 ** 32)],
    ["u32 -1", (w) => w.writeU32(-1)],
    ["u32 0.5", (w) => w.writeU32(0.5)],
    ["u32 Infinity", (w) => w.writeU32(Number.POSITIVE_INFINITY)],
    ["i32 2^31", (w) => w.writeI32(2 ** 31)],
    ["i32 -2^31-1", (w) => w.writeI32(-(2 ** 31) - 1)],
    ["i32 NaN", (w) => w.writeI32(Number.NaN)],
    ["u64 2^64", (w) => w.writeU64(2n ** 64n)],
    ["u64 -1", (w) => w.writeU64(-1n)],
    ["i64 2^63", (w) => w.writeI64(2n ** 63n)],
    ["i64 -2^63-1", (w) => w.writeI64(-(2n ** 63n) - 1n)],
    ["u64Number 2^53", (w) => w.writeU64Number(2 ** 53)],
    ["u64Number -1", (w) => w.writeU64Number(-1)],
    ["u64Number 1.5", (w) => w.writeU64Number(1.5)],
    ["u64Number NaN", (w) => w.writeU64Number(Number.NaN)],
    ["u64Number Infinity", (w) => w.writeU64Number(Number.POSITIVE_INFINITY)],
    ["i64Number 2^53", (w) => w.writeI64Number(2 ** 53)],
    ["i64Number -2^53", (w) => w.writeI64Number(-(2 ** 53))],
    ["i64Number 0.1", (w) => w.writeI64Number(0.1)],
    ["len -1", (w) => w.writeLen(-1)],
    ["len 1.5", (w) => w.writeLen(1.5)],
    ["len NaN", (w) => w.writeLen(Number.NaN)],
    ["uuid too short", (w) => w.writeUuid("123e4567")],
    ["uuid bad hex", (w) => w.writeUuid("123e4567-e89b-12d3-a456-42661417400g")],
    ["uuid no hyphens", (w) => w.writeUuid("123e4567e89b12d3a456426614174000    ")],
  ];
  it.each(rejects)("%s throws RangeError and writes nothing", (_name, fn) => {
    const w = new KeelWriter();
    w.writeU8(7);
    expect(() => fn(w)).toThrow(RangeError);
    expect(w.position).toBe(1);
  });

  it("len above u32 is length_too_large", () => {
    expectWireError(() => new KeelWriter().writeLen(2 ** 32), "length_too_large", { len: 2 ** 32 });
  });

  it("accepts every boundary", () => {
    const w = new KeelWriter();
    w.writeU8(0);
    w.writeU8(255);
    w.writeI8(-128);
    w.writeI8(127);
    w.writeU16(0);
    w.writeU16(65535);
    w.writeI16(-32768);
    w.writeI16(32767);
    w.writeU32(0);
    w.writeU32(2 ** 32 - 1);
    w.writeI32(-(2 ** 31));
    w.writeI32(2 ** 31 - 1);
    w.writeU64(0n);
    w.writeU64(2n ** 64n - 1n);
    w.writeI64(-(2n ** 63n));
    w.writeI64(2n ** 63n - 1n);
    expect(w.position).toBe(1 + 1 + 1 + 1 + 2 + 2 + 2 + 2 + 4 * 4 + 8 * 4);
  });

  it("rejects an invalid initial capacity", () => {
    expect(() => new KeelWriter(-1)).toThrow(RangeError);
    expect(() => new KeelWriter(1.5)).toThrow(RangeError);
  });
});

describe("KeelWriter number-backed 64-bit integers", () => {
  it("writeU64Number matches writeU64 at the edges", () => {
    for (const n of [0, 1, 255, 2 ** 32 - 1, 2 ** 32, 2 ** 32 + 1, 2 ** 40, Number.MAX_SAFE_INTEGER]) {
      expect(written((w) => w.writeU64Number(n))).toBe(written((w) => w.writeU64(BigInt(n))));
    }
    expect(written((w) => w.writeU64Number(Number.MAX_SAFE_INTEGER))).toBe("ffffffffffff1f00");
  });

  it("writeI64Number matches writeI64 at the edges", () => {
    const values = [
      0,
      1,
      -1,
      -2,
      2 ** 31,
      -(2 ** 31),
      2 ** 32 - 1,
      2 ** 32,
      -(2 ** 32),
      -(2 ** 32) - 1,
      2 ** 40 + 12345,
      -(2 ** 40) - 12345,
      Number.MAX_SAFE_INTEGER,
      Number.MIN_SAFE_INTEGER,
    ];
    for (const n of values) {
      expect(written((w) => w.writeI64Number(n))).toBe(written((w) => w.writeI64(BigInt(n))));
    }
  });

  it("agrees with the bigint writers on random safe integers", () => {
    const rand = prng(11);
    for (let i = 0; i < 2000; i++) {
      const magnitude = 2 ** randInt(rand, 54);
      const n = Math.floor(rand() * magnitude) * (rand() < 0.5 ? -1 : 1);
      if (!Number.isSafeInteger(n)) continue;
      expect(written((w) => w.writeI64Number(n))).toBe(written((w) => w.writeI64(BigInt(n))));
      if (n >= 0) {
        expect(written((w) => w.writeU64Number(n))).toBe(written((w) => w.writeU64(BigInt(n))));
      }
    }
  });
});

describe("KeelWriter strings", () => {
  const encoder = new TextEncoder();
  const expectedFor = (s: string): string => {
    const utf8 = encoder.encode(s);
    const prefix = new Uint8Array(4);
    new DataView(prefix.buffer).setUint32(0, utf8.length, true);
    return toHex(prefix) + toHex(utf8);
  };

  const samples: [string, string][] = [
    ["empty", ""],
    ["ascii", "hello"],
    ["latin-1", "café crème brûlée"],
    ["two byte boundary", "߿ࠀ"],
    ["bmp cjk", "日本語のテキスト"],
    ["emoji", "\u{1f30a}\u{1f600}\u{1f468}‍\u{1f469}‍\u{1f467}"],
    ["nul", "a\u0000b"],
    ["bom", "﻿abc"],
    ["max bmp", "￿￾"],
    ["max code point", "\u{10ffff}"],
    ["lone high surrogate", "a\ud800b"],
    ["lone low surrogate", "a\udc00b"],
    ["high surrogate at end", "abc\ud83d"],
    ["reversed pair", "\udc00\ud800"],
  ];
  it.each(samples)("matches TextEncoder: %s", (_name, s) => {
    expect(written((w) => w.writeStr(s))).toBe(expectedFor(s));
  });

  it("matches TextEncoder for lengths straddling the short-string threshold", () => {
    const alphabet = ["a", "é", "日", "\u{1f30a}", "\ud800", "Z"];
    const rand = prng(3);
    for (let len = 0; len <= 130; len++) {
      let s = "";
      // Build by UTF-16 units: alternate alphabet entries until the length is reached exactly.
      while (s.length < len) {
        const piece = alphabet[randInt(rand, alphabet.length)] as string;
        s += s.length + piece.length <= len ? piece : "x";
      }
      expect(s.length).toBe(len);
      expect(written((w) => w.writeStr(s))).toBe(expectedFor(s));
    }
  });

  it("handles a surrogate pair on either side of the threshold", () => {
    for (const before of [44, 45, 46, 47, 48, 49, 50]) {
      const s = "a".repeat(before) + "\u{1f30a}" + "b";
      expect(written((w) => w.writeStr(s))).toBe(expectedFor(s));
    }
  });

  it("encodes long strings whose UTF-8 is larger than the optimistic estimate", () => {
    for (const s of [
      "é".repeat(10_000),
      "日".repeat(7_777),
      "\u{1f30a}".repeat(5_000),
      "a".repeat(100_000),
      `${"a".repeat(1000)}日${"b".repeat(1000)}`,
      `${"a".repeat(100)}\ud800${"b".repeat(100)}`,
      `${"é".repeat(500)}\ud83d`,
    ]) {
      expect(written((w) => w.writeStr(s))).toBe(expectedFor(s));
    }
  });

  it("keeps previously written data when a long string forces a grow", () => {
    const w = new KeelWriter(8);
    w.writeU32(0xdeadbeef);
    const s = "é".repeat(1000);
    w.writeStr(s);
    w.writeU8(9);
    const hex = toHex(w.finish());
    expect(hex).toBe(`efbeadde${expectedFor(s)}09`);
  });
});

describe("KeelWriter buffer management", () => {
  it("grows geometrically", () => {
    const w = new KeelWriter(0);
    const capacities = new Set<number>();
    for (let i = 0; i < 100_000; i++) {
      w.writeU8(i & 0xff);
      capacities.add(w.capacity);
    }
    // 64 -> 128 -> ... -> 131072: about 12 reallocations, not thousands.
    expect(capacities.size).toBeLessThanOrEqual(14);
    expect(w.position).toBe(100_000);
    const sorted = [...capacities].sort((a, b) => a - b);
    for (let i = 1; i < sorted.length; i++) {
      expect(sorted[i] as number).toBeGreaterThanOrEqual((sorted[i - 1] as number) * 2);
    }
  });

  it("does not reallocate while capacity suffices", () => {
    const w = new KeelWriter(64);
    for (let i = 0; i < 16; i++) w.writeU32(i);
    expect(w.capacity).toBe(64);
    w.writeStr("short"); // 9 bytes, forces a grow
    expect(w.capacity).toBeGreaterThan(64);
  });

  it("reserve makes room for a burst of writes", () => {
    const w = new KeelWriter(0);
    w.reserve(1000);
    const capacity = w.capacity;
    expect(capacity).toBeGreaterThanOrEqual(1000);
    for (let i = 0; i < 250; i++) w.writeU32(i);
    expect(w.capacity).toBe(capacity);
    expect(w.position).toBe(1000);
  });

  it("reserve accounts for bytes already written", () => {
    const w = new KeelWriter(16);
    w.writeRaw(new Uint8Array(10));
    w.reserve(20);
    expect(w.capacity).toBeGreaterThanOrEqual(30);
  });

  it("reserve rejects nonsense and absurd sizes", () => {
    const w = new KeelWriter();
    expect(() => w.reserve(-1)).toThrow(RangeError);
    expect(() => w.reserve(1.5)).toThrow(RangeError);
    expectWireError(() => w.reserve(2 ** 32), "length_too_large");
  });

  it("works from a zero-capacity start", () => {
    const w = new KeelWriter(0);
    expect(w.capacity).toBe(0);
    expect(toHex(w.finish())).toBe("");
    w.writeU8(1);
    expect(toHex(w.finish())).toBe("01");
  });

  it("finish returns an exact-length view", () => {
    const w = new KeelWriter(1024);
    w.writeU32(1);
    const out = w.finish();
    expect(out).toBeInstanceOf(Uint8Array);
    expect(out.byteLength).toBe(4);
    expect(out.length).toBe(4);
    expect(toHex(out)).toBe("01000000");
  });

  it("finish hands over the buffer and resets the writer", () => {
    const w = new KeelWriter();
    w.writeU32(0x11111111);
    const first = w.finish();
    expect(w.position).toBe(0);
    w.writeU32(0x22222222);
    const second = w.finish();
    expect(toHex(first)).toBe("11111111");
    expect(toHex(second)).toBe("22222222");
    expect(first.buffer).not.toBe(second.buffer);
  });

  it("finish on an empty writer yields empty bytes", () => {
    expect(new KeelWriter().finish().length).toBe(0);
  });

  it("writeBytes writes only the viewed range of a larger buffer", () => {
    const big = new Uint8Array([0, 1, 2, 3, 4, 5]);
    expect(written((w) => w.writeBytes(big.subarray(2, 4)))).toBe("020000000203");
  });

  it("writeRaw copies a sub-range without a prefix", () => {
    const src = new Uint8Array([1, 2, 3, 4, 5]);
    expect(written((w) => w.writeRaw(src, 1, 4))).toBe("020304");
    expect(written((w) => w.writeRaw(src, 5))).toBe("");
    expect(() => new KeelWriter().writeRaw(src, 3, 2)).toThrow(RangeError);
    expect(() => new KeelWriter().writeRaw(src, 0, 6)).toThrow(RangeError);
    expect(() => new KeelWriter().writeRaw(src, -1)).toThrow(RangeError);
  });

  it("view exposes written bytes and rewind discards them", () => {
    const w = new KeelWriter();
    w.writeRaw(fromHex("0102030405"));
    expect(toHex(w.view(1, 4))).toBe("020304");
    expect(toHex(w.view(2))).toBe("030405");
    w.rewind(2);
    expect(w.position).toBe(2);
    w.writeU8(9);
    expect(toHex(w.finish())).toBe("010209");
    expect(() => new KeelWriter().view(0, 1)).toThrow(RangeError);
    expect(() => new KeelWriter().rewind(1)).toThrow(RangeError);
    expect(() => new KeelWriter().rewind(-1)).toThrow(RangeError);
  });

  it("copes with writeRaw of a view over its own buffer", () => {
    const w = new KeelWriter(8);
    w.writeRaw(fromHex("0102030405060708"));
    w.writeRaw(w.view(2, 6)); // forces a grow while the source aliases the old buffer
    expect(toHex(w.finish())).toBe("010203040506070803040506");
  });
});
