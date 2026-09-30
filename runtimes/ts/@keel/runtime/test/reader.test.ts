import { describe, expect, it } from "vitest";
import { KeelReader } from "../src/wire/reader.js";
import { KeelWriter } from "../src/wire/writer.js";
import { catchWireError, embedded, expectWireError, fromHex, prng, randInt, toHex } from "./helpers.js";

const reader = (hex: string): KeelReader => new KeelReader(fromHex(hex));

describe("KeelReader primitives", () => {
  const table: [string, string, (r: KeelReader) => unknown, unknown][] = [
    ["u8", "ff", (r) => r.readU8(), 255],
    ["i8 -128", "80", (r) => r.readI8(), -128],
    ["i8 127", "7f", (r) => r.readI8(), 127],
    ["u16", "3412", (r) => r.readU16(), 0x1234],
    ["i16 min", "0080", (r) => r.readI16(), -32768],
    ["u32", "78563412", (r) => r.readU32(), 0x12345678],
    ["u32 max", "ffffffff", (r) => r.readU32(), 0xffffffff],
    ["i32 -2", "feffffff", (r) => r.readI32(), -2],
    ["i32 min", "00000080", (r) => r.readI32(), -2147483648],
    ["u64 max", "ffffffffffffffff", (r) => r.readU64(), 2n ** 64n - 1n],
    ["i64 min", "0000000000000080", (r) => r.readI64(), -(2n ** 63n)],
    ["i64 -(2^53+1)", "ffffffffffffdfff", (r) => r.readI64(), -9007199254740993n],
    ["f32", "c3f54840", (r) => r.readF32(), Math.fround(3.14)],
    ["f64", "6957148b0abf0540", (r) => r.readF64(), Math.E],
    ["bool true", "01", (r) => r.readBool(), true],
    ["bool false", "00", (r) => r.readBool(), false],
    ["u64Number", "ffffffffffff1f00", (r) => r.readU64Number(), Number.MAX_SAFE_INTEGER],
    ["u64Number 2^32", "0000000001000000", (r) => r.readU64Number(), 2 ** 32],
    ["i64Number -1", "ffffffffffffffff", (r) => r.readI64Number(), -1],
    ["i64Number min safe", "01000000 0000e0ff", (r) => r.readI64Number(), Number.MIN_SAFE_INTEGER],
    ["i64Number max safe", "ffffffff ffff1f00", (r) => r.readI64Number(), Number.MAX_SAFE_INTEGER],
    ["uuid", "123e4567e89b12d3a456426614174000", (r) => r.readUuid(), "123e4567-e89b-12d3-a456-426614174000"],
    ["len", "03000000 aabbcc", (r) => r.readLen(), 3],
  ];
  it.each(table)("%s", (_name, hex, read, expected) => {
    expect(read(reader(hex))).toBe(expected);
  });

  it("reads -0 and NaN floats faithfully", () => {
    expect(Object.is(reader("0000000000000080").readF64(), -0)).toBe(true);
    expect(Number.isNaN(reader("0000c07f").readF32())).toBe(true);
  });

  it("tracks position and remaining", () => {
    const r = reader("01 0200 03000000 aa");
    expect(r.position).toBe(0);
    expect(r.remaining).toBe(8);
    r.readU8();
    r.readU16();
    expect(r.position).toBe(3);
    expect(r.remaining).toBe(5);
    r.readU32();
    r.readU8();
    expect(r.remaining).toBe(0);
    r.finish();
  });

  it("respects byteOffset and byteLength of the input view", () => {
    const bytes = embedded(fromHex("0100 07000000 05000000 68656c6c6f"));
    expect(bytes.byteOffset).toBeGreaterThan(0);
    const r = new KeelReader(bytes);
    expect(r.readU16()).toBe(1);
    expect(r.readU32()).toBe(7);
    expect(r.readStr()).toBe("hello");
    expect(r.remaining).toBe(0);
    r.finish();
    // The junk after the view is unreachable.
    expect(catchWireError(() => new KeelReader(bytes).readRaw(bytes.length + 1)).code).toBe("unexpected_eof");
  });

  it("reads back what the writer wrote, in a view with an offset", () => {
    const w = new KeelWriter();
    w.writeI64(-5n);
    w.writeStr("日本");
    w.writeUuid("00000000-0000-0000-0000-000000000000");
    const r = new KeelReader(embedded(w.finish()));
    expect(r.readI64()).toBe(-5n);
    expect(r.readStr()).toBe("日本");
    expect(r.readUuid()).toBe("00000000-0000-0000-0000-000000000000");
    r.finish();
  });
});

describe("KeelReader number-backed 64-bit integers", () => {
  it("rejects u64 above the safe range", () => {
    expectWireError(() => reader("0000000000002000").readU64Number(), "unsafe_integer", {
      value: 2n ** 53n,
      at: 0,
    });
    expectWireError(() => reader("ffffffffffffffff").readU64Number(), "unsafe_integer", {
      value: 2n ** 64n - 1n,
    });
  });

  it("rejects i64 outside the safe range", () => {
    expectWireError(() => reader("0000000000002000").readI64Number(), "unsafe_integer", { value: 2n ** 53n });
    expectWireError(() => reader("00000000 0000e0ff").readI64Number(), "unsafe_integer", {
      value: -(2n ** 53n),
    });
    expectWireError(() => reader("0000000000000080").readI64Number(), "unsafe_integer", {
      value: -(2n ** 63n),
    });
    expectWireError(() => reader("ffffffffffffff7f").readI64Number(), "unsafe_integer");
  });

  it("does not consume the bytes of a rejected value", () => {
    const r = reader("0000000000002000");
    expect(() => r.readU64Number()).toThrow();
    expect(r.position).toBe(0);
  });

  it("agrees with the bigint readers on random values", () => {
    const rand = prng(21);
    for (let i = 0; i < 2000; i++) {
      const w = new KeelWriter();
      const n = Math.floor(rand() * 2 ** randInt(rand, 54)) * (rand() < 0.5 ? -1 : 1) + 0; // + 0 turns -0 into 0
      if (!Number.isSafeInteger(n)) continue;
      w.writeI64(BigInt(n));
      const bytes = w.finish();
      expect(new KeelReader(bytes).readI64Number()).toBe(n);
      expect(BigInt(new KeelReader(bytes).readI64Number())).toBe(new KeelReader(bytes).readI64());
      if (n >= 0) expect(new KeelReader(bytes).readU64Number()).toBe(n);
    }
  });
});

describe("KeelReader unexpected_eof", () => {
  const readers: [string, number, (r: KeelReader) => unknown][] = [
    ["u8", 1, (r) => r.readU8()],
    ["i8", 1, (r) => r.readI8()],
    ["u16", 2, (r) => r.readU16()],
    ["i16", 2, (r) => r.readI16()],
    ["u32", 4, (r) => r.readU32()],
    ["i32", 4, (r) => r.readI32()],
    ["u64", 8, (r) => r.readU64()],
    ["i64", 8, (r) => r.readI64()],
    ["u64Number", 8, (r) => r.readU64Number()],
    ["i64Number", 8, (r) => r.readI64Number()],
    ["f32", 4, (r) => r.readF32()],
    ["f64", 8, (r) => r.readF64()],
    ["bool", 1, (r) => r.readBool()],
    ["len", 4, (r) => r.readLen()],
    ["str", 4, (r) => r.readStr()],
    ["bytes", 4, (r) => r.readBytes()],
    ["uuid", 16, (r) => r.readUuid()],
  ];

  it.each(readers)("%s on empty input", (_name, width, read) => {
    expectWireError(() => read(new KeelReader(new Uint8Array(0))), "unexpected_eof", { needed: width, at: 0 });
  });

  it.each(readers)("%s one byte short", (_name, width, read) => {
    expectWireError(() => read(new KeelReader(new Uint8Array(width - 1))), "unexpected_eof", {
      needed: 1,
      at: 0,
    });
  });

  it("reports the offset where the read started and the shortfall", () => {
    const r = reader("0102 03");
    r.readU16();
    expectWireError(() => r.readU32(), "unexpected_eof", { at: 2, needed: 3 });
  });

  it("readRaw reports the shortfall", () => {
    const r = reader("010203");
    expectWireError(() => r.readRaw(10), "unexpected_eof", { needed: 7, at: 0 });
    expect(() => r.readRaw(-1)).toThrow(RangeError);
    expect(() => r.readRaw(1.5)).toThrow(RangeError);
  });
});

describe("KeelReader validation", () => {
  it("rejects bool bytes other than 0 and 1", () => {
    for (const v of [2, 3, 127, 255]) {
      expectWireError(() => new KeelReader(new Uint8Array([v])).readBool(), "invalid_tag", {
        tag: v,
        at: 0,
        ty: "bool",
      });
    }
  });

  it("readLen accepts a count equal to the remaining bytes", () => {
    expect(reader("02000000 0102").readLen()).toBe(2);
    expect(reader("00000000").readLen()).toBe(0);
  });

  it("readLen rejects a count larger than the remaining bytes", () => {
    expectWireError(() => reader("03000000 0102").readLen(), "length_too_large", { len: 3, at: 0 });
    expectWireError(() => reader("ffffffff").readLen(), "length_too_large", { len: 0xffffffff, at: 0 });
  });

  it("readLen scales the bound by the minimum item size", () => {
    expect(reader("02000000 0102030405060708").readLen(4)).toBe(2);
    expectWireError(() => reader("03000000 0102030405060708").readLen(4), "length_too_large", { len: 3 });
    expect(reader("00000000").readLen(1000)).toBe(0);
  });

  it("readLen reports the offset of the count", () => {
    const r = reader("aa 09000000");
    r.readU8();
    expectWireError(() => r.readLen(), "length_too_large", { at: 1 });
  });

  it("readStr and readBytes reject a length beyond the input", () => {
    expectWireError(() => reader("05000000 6162").readStr(), "length_too_large", { len: 5 });
    expectWireError(() => reader("05000000 6162").readBytes(), "length_too_large", { len: 5 });
  });

  it("finish reports the number of trailing bytes", () => {
    const r = reader("0102030405");
    r.readU8();
    expectWireError(() => r.finish(), "trailing_bytes", { count: 4 });
    new KeelReader(new Uint8Array(0)).finish();
  });
});

describe("KeelReader.readStr", () => {
  it("decodes unicode", () => {
    expect(reader("0b00000068c3a96c6c6f20f09f8c8a").readStr()).toBe("héllo \u{1f30a}");
    expect(reader("00000000").readStr()).toBe("");
  });

  it("keeps a leading byte order mark", () => {
    expect(reader("04000000 efbbbf61").readStr()).toBe("﻿a");
    expect(reader("03000000 efbbbf").readStr()).toBe("﻿");
  });

  it("keeps embedded NUL characters", () => {
    expect(reader("03000000 610062").readStr()).toBe("a\u0000b");
  });

  const invalid: [string, string][] = [
    ["lone continuation byte", "80"],
    ["lone lead byte", "c3"],
    ["truncated 3-byte sequence", "e282"],
    ["truncated 4-byte sequence", "f09f8c"],
    ["overlong 2-byte NUL", "c080"],
    ["overlong 3-byte", "e08080"],
    ["encoded high surrogate", "eda080"],
    ["encoded low surrogate", "edb080"],
    ["above U+10FFFF", "f4908080"],
    ["invalid lead byte f8", "f8888080 80"],
    ["invalid byte ff", "ff"],
    ["bad continuation", "c328"],
  ];
  it.each(invalid)("rejects %s", (_name, payloadHex) => {
    const payload = fromHex(payloadHex);
    const w = new KeelWriter();
    w.writeLen(payload.length);
    w.writeRaw(payload);
    expectWireError(() => new KeelReader(w.finish()).readStr(), "invalid_utf8", { at: 0 });
  });

  it("reports the offset of the string, not of the bad byte", () => {
    const r = reader("aa bb 02000000 c328");
    r.readU16();
    expectWireError(() => r.readStr(), "invalid_utf8", { at: 2 });
  });
});

describe("KeelReader byte views", () => {
  it("readBytes returns a borrowed view of the input, not a copy", () => {
    const input = fromHex("03000000 010203 ff");
    const r = new KeelReader(input);
    const view = r.readBytes();
    expect(toHex(view)).toBe("010203");
    expect(view.buffer).toBe(input.buffer);
    expect(view.byteOffset).toBe(input.byteOffset + 4);
    // Borrowed: a change to the input is visible through the view.
    input[4] = 0x99;
    expect(view[0]).toBe(0x99);
    // Owning it requires an explicit copy.
    const owned = view.slice();
    input[4] = 0x01;
    expect(owned[0]).toBe(0x99);
  });

  it("readBytes on an empty byte string returns an empty view", () => {
    expect(reader("00000000").readBytes().length).toBe(0);
  });

  it("readRest returns everything left and moves to the end", () => {
    const r = reader("0102030405");
    r.readU8();
    expect(toHex(r.readRest())).toBe("02030405");
    expect(r.remaining).toBe(0);
    expect(r.readRest().length).toBe(0);
    r.finish();
  });

  it("readRaw returns an exact view", () => {
    const r = reader("0102030405");
    expect(toHex(r.readRaw(3))).toBe("010203");
    expect(toHex(r.readRaw(0))).toBe("");
    expect(r.position).toBe(3);
  });
});
