import { describe, expect, it } from "vitest";
import { type Codec, type WireResult, codecs, decodeValue, encodeValue } from "../src/wire/codec.js";
import { UndraReader } from "../src/wire/reader.js";
import { UndraWriter } from "../src/wire/writer.js";
import { type Filter, type Shape, filterCodec, shapeCodec, todoCodec } from "./fixtures.js";
import { embedded, expectWireError, fromHex, prng, randInt, randomBytes, toHex } from "./helpers.js";

/** Encodes and decodes every value, from a plain and from an offset buffer, and checks re-encoding is identical. */
function roundTrips<T>(name: string, codec: Codec<T>, values: readonly T[]): void {
  it(`${name} round-trips`, () => {
    for (const value of values) {
      const bytes = encodeValue(codec, value);
      expect(decodeValue(codec, bytes)).toEqual(value);
      expect(decodeValue(codec, embedded(bytes))).toEqual(value);
      expect(toHex(encodeValue(codec, decodeValue(codec, bytes)))).toBe(toHex(bytes));
    }
  });
}

const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;
const U64_MAX = 2n ** 64n - 1n;
const BEYOND_SAFE = BigInt(Number.MAX_SAFE_INTEGER) + 2n;

describe("primitive codecs", () => {
  roundTrips("bool", codecs.bool, [true, false]);
  roundTrips("u8", codecs.u8, [0, 1, 127, 128, 255]);
  roundTrips("i8", codecs.i8, [-128, -1, 0, 1, 127]);
  roundTrips("u16", codecs.u16, [0, 1, 255, 256, 65535]);
  roundTrips("i16", codecs.i16, [-32768, -1, 0, 1, 32767]);
  roundTrips("u32", codecs.u32, [0, 1, 65536, 2147483648, 4294967295]);
  roundTrips("i32", codecs.i32, [-2147483648, -1, 0, 1, 2147483647]);
  roundTrips("u64", codecs.u64, [0n, 1n, 2n ** 32n, BEYOND_SAFE, U64_MAX]);
  roundTrips("i64", codecs.i64, [I64_MIN, -BEYOND_SAFE, -1n, 0n, 1n, BEYOND_SAFE, I64_MAX]);
  roundTrips("u64Number", codecs.u64Number, [0, 1, 2 ** 32, Number.MAX_SAFE_INTEGER]);
  roundTrips("i64Number", codecs.i64Number, [Number.MIN_SAFE_INTEGER, -(2 ** 32), -1, 0, 1, Number.MAX_SAFE_INTEGER]);
  roundTrips("f32", codecs.f32, [
    0,
    -0,
    1.5,
    Math.fround(3.14),
    3.4028234663852886e38,
    1.401298464324817e-45,
    Number.POSITIVE_INFINITY,
    Number.NEGATIVE_INFINITY,
    Number.NaN,
  ]);
  roundTrips("f64", codecs.f64, [
    0,
    -0,
    Math.PI,
    Number.MAX_VALUE,
    Number.MIN_VALUE,
    Number.EPSILON,
    Number.POSITIVE_INFINITY,
    Number.NEGATIVE_INFINITY,
    Number.NaN,
  ]);
  roundTrips("unit", codecs.unit, [undefined]);
  roundTrips("handle", codecs.handle, [0n, 4294967297n, U64_MAX]);

  it("i64 beyond Number.MAX_SAFE_INTEGER keeps every bit", () => {
    for (const v of [BEYOND_SAFE, -BEYOND_SAFE, 9007199254740993n, I64_MAX - 1n, I64_MIN + 1n]) {
      expect(decodeValue(codecs.i64, encodeValue(codecs.i64, v))).toBe(v);
    }
    expect(toHex(encodeValue(codecs.i64, -9007199254740993n))).toBe("ffffffffffffdfff");
  });

  it("u64 and i64 reject values outside their range", () => {
    expect(() => encodeValue(codecs.u64, -1n)).toThrow(RangeError);
    expect(() => encodeValue(codecs.u64, U64_MAX + 1n)).toThrow(RangeError);
    expect(() => encodeValue(codecs.i64, I64_MAX + 1n)).toThrow(RangeError);
    expect(() => encodeValue(codecs.i64, I64_MIN - 1n)).toThrow(RangeError);
  });

  it("unit occupies no bytes", () => {
    expect(encodeValue(codecs.unit, undefined).length).toBe(0);
    expect(decodeValue(codecs.unit, new Uint8Array(0))).toBeUndefined();
    expectWireError(() => decodeValue(codecs.unit, new Uint8Array(1)), "trailing_bytes", { count: 1 });
  });

  it("the js_number codecs fail with unsafe_integer for larger wire values", () => {
    expectWireError(() => decodeValue(codecs.u64Number, encodeValue(codecs.u64, 2n ** 53n)), "unsafe_integer");
    expectWireError(() => decodeValue(codecs.i64Number, encodeValue(codecs.i64, I64_MIN)), "unsafe_integer");
    expect(() => encodeValue(codecs.u64Number, 2 ** 53)).toThrow(RangeError);
  });

  it("encode is little-endian and unpadded", () => {
    const w = new UndraWriter();
    codecs.u8.encode(w, 1);
    codecs.u32.encode(w, 2);
    codecs.u16.encode(w, 3);
    expect(toHex(w.finish())).toBe("01" + "02000000" + "0300");
  });
});

describe("string codec", () => {
  roundTrips("string", codecs.string, [
    "",
    "a",
    "hello world",
    "héllo \u{1f30a}",
    "日本語",
    "\u{1f468}‍\u{1f469}‍\u{1f467}‍\u{1f466}",
    "﻿",
    "﻿starts with a BOM",
    "a\u0000b\u0000",
    "\u{10ffff}",
    "￿",
    "x".repeat(10_000),
    "é".repeat(10_000),
    "\u{1f30a}".repeat(3_000),
    "line1\nline2\r\nline3\ttab",
  ]);

  it("replaces lone surrogates with U+FFFD, as the encoder of a Rust String would never see them", () => {
    expect(decodeValue(codecs.string, encodeValue(codecs.string, "a\ud800b"))).toBe("a�b");
  });
});

describe("bytes codec", () => {
  const all = new Uint8Array(256).map((_, i) => i);
  roundTrips("bytes", codecs.bytes, [
    new Uint8Array(0),
    new Uint8Array([0]),
    new Uint8Array([1, 2, 3, 255]),
    all,
    new Uint8Array(100_000).fill(0xab),
  ]);

  it("decodes to an owned copy that survives changes to the input", () => {
    const input = encodeValue(codecs.bytes, new Uint8Array([1, 2, 3]));
    const out = decodeValue(codecs.bytes, input);
    expect(out.buffer).not.toBe(input.buffer);
    input.fill(0);
    expect(toHex(out)).toBe("010203");
  });

  it("encodes a view of a larger buffer as only the viewed bytes", () => {
    expect(toHex(encodeValue(codecs.bytes, all.subarray(1, 4)))).toBe("03000000010203");
  });
});

describe("duration codec", () => {
  roundTrips("duration", codecs.duration, [0, 1, 1500, 0.5, 0.000001, 86_400_000, 1e9, 9_223_372_036_854]);

  it("is i64 nanoseconds", () => {
    expect(toHex(encodeValue(codecs.duration, 1500))).toBe("002f685900000000");
    expect(toHex(encodeValue(codecs.duration, 0))).toBe("0000000000000000");
  });

  it("rejects a negative count when decoding and a negative duration when encoding", () => {
    expectWireError(() => decodeValue(codecs.duration, encodeValue(codecs.i64, -1n)), "negative_duration", {
      nanos: -1n,
    });
    expectWireError(() => decodeValue(codecs.duration, encodeValue(codecs.i64, I64_MIN)), "negative_duration");
    expectWireError(() => encodeValue(codecs.duration, -1), "negative_duration");
  });

  it("decodes the largest count", () => {
    const ms = decodeValue(codecs.duration, encodeValue(codecs.i64, I64_MAX));
    expect(Math.trunc(ms)).toBe(9_223_372_036_854);
  });
});

describe("timestamp codec", () => {
  roundTrips("timestamp", codecs.timestamp, [
    0,
    1727654400000,
    -1,
    -86_400_000,
    Number.MAX_SAFE_INTEGER,
    Number.MIN_SAFE_INTEGER,
  ]);

  it("is i64 milliseconds", () => {
    expect(toHex(encodeValue(codecs.timestamp, 1727654400000))).toBe("00103a4092010000");
  });

  it("rejects fractional milliseconds instead of truncating them", () => {
    expect(() => encodeValue(codecs.timestamp, 1.5)).toThrow(RangeError);
    expect(() => encodeValue(codecs.timestamp, Number.NaN)).toThrow(RangeError);
  });

  it("fails with unsafe_integer beyond the safe range", () => {
    expectWireError(() => decodeValue(codecs.timestamp, encodeValue(codecs.i64, I64_MAX)), "unsafe_integer");
  });
});

describe("uuid codec", () => {
  roundTrips("uuid", codecs.uuid, [
    "00000000-0000-0000-0000-000000000000",
    "ffffffff-ffff-ffff-ffff-ffffffffffff",
    "123e4567-e89b-12d3-a456-426614174000",
    "0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d",
  ]);

  it("is 16 raw bytes", () => {
    expect(toHex(encodeValue(codecs.uuid, "123e4567-e89b-12d3-a456-426614174000"))).toBe(
      "123e4567e89b12d3a456426614174000",
    );
  });

  it("round-trips random UUIDs inside a larger message", () => {
    const rand = prng(31);
    const codec = codecs.vec(codecs.uuid);
    const uuids: string[] = [];
    for (let i = 0; i < 200; i++) {
      const b = randomBytes(rand, 16);
      const hex = toHex(b);
      uuids.push(`${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`);
    }
    expect(decodeValue(codec, encodeValue(codec, uuids))).toEqual(uuids);
  });
});

describe("option", () => {
  roundTrips("option<string>", codecs.option(codecs.string), [null, "", "x", "\u{1f30a}"]);
  roundTrips("option<u32>", codecs.option(codecs.u32), [null, 0, 4294967295]);
  roundTrips("option<vec<u8>>", codecs.option(codecs.vec(codecs.u8)), [null, [], [1, 2, 3]]);

  it("uses tag 0 for None and 1 for Some", () => {
    const c = codecs.option(codecs.string);
    expect(toHex(encodeValue(c, null))).toBe("00");
    expect(toHex(encodeValue(c, "x"))).toBe("010100000078");
  });

  it("a Some of a unit is distinguishable from None", () => {
    const c = codecs.option(codecs.unit);
    expect(toHex(encodeValue(c, undefined))).toBe("01");
    expect(decodeValue(c, fromHex("01"))).toBeUndefined();
    expect(decodeValue(c, fromHex("00"))).toBeNull();
  });

  it("cannot tell Some(None) from None (documented limitation of T | null)", () => {
    const c = codecs.option(codecs.option(codecs.u8));
    expect(toHex(encodeValue(c, null))).toBe("00");
    expect(decodeValue(c, fromHex("0100"))).toBeNull();
    expect(decodeValue(c, fromHex("00"))).toBeNull();
    expect(decodeValue(c, fromHex("010107"))).toBe(7);
  });

  it("rejects a tag other than 0 or 1", () => {
    expectWireError(() => decodeValue(codecs.option(codecs.u8), fromHex("02")), "invalid_tag", {
      tag: 2,
      at: 0,
      ty: "Option",
    });
  });
});

describe("vec", () => {
  roundTrips("vec<i32>", codecs.vec(codecs.i32), [[], [1, -1, 7], Array.from({ length: 1000 }, (_, i) => i - 500)]);
  roundTrips("vec<string>", codecs.vec(codecs.string), [[], [""], ["a", "日本", "\u{1f30a}"]]);
  roundTrips("vec<vec<u8>>", codecs.vec(codecs.vec(codecs.u8)), [[], [[]], [[1], [], [2, 3]]]);
  roundTrips("vec<option<i64>>", codecs.vec(codecs.option(codecs.i64)), [[null, 5n, null, I64_MIN]]);
  roundTrips("vec<bytes>", codecs.vec(codecs.bytes), [[], [new Uint8Array(0), new Uint8Array([9])]]);
  roundTrips("vec<todo>", codecs.vec(todoCodec), [
    [],
    [{ id: "123e4567-e89b-12d3-a456-426614174000", title: "Milk", done: false }],
  ]);

  it("writes a u32 count and then the items", () => {
    expect(toHex(encodeValue(codecs.vec(codecs.i32), [1, -1, 7]))).toBe("0300000001000000ffffffff07000000");
    expect(toHex(encodeValue(codecs.vec(codecs.string), []))).toBe("00000000");
  });

  it("rejects a count larger than the remaining input without allocating for it", () => {
    expectWireError(() => decodeValue(codecs.vec(codecs.u8), fromHex("ffffffff 01")), "length_too_large", {
      len: 0xffffffff,
      at: 0,
    });
    expectWireError(() => decodeValue(codecs.vec(codecs.string), fromHex("05000000 00000000")), "length_too_large");
  });

  it("propagates an item failure with the item's own offset", () => {
    expectWireError(() => decodeValue(codecs.vec(codecs.bool), fromHex("02000000 01 05")), "invalid_tag", {
      tag: 5,
      at: 5,
      ty: "bool",
    });
    expectWireError(() => decodeValue(codecs.vec(codecs.u32), fromHex("02000000 01000000 02")), "unexpected_eof");
  });

  it("cannot count zero-width items beyond the remaining input (documented limitation)", () => {
    const c = codecs.vec(codecs.unit);
    expect(decodeValue(c, fromHex("00000000"))).toEqual([]);
    expectWireError(() => decodeValue(c, fromHex("03000000")), "length_too_large", { len: 3 });
  });
});

describe("map", () => {
  const strI32 = codecs.map(codecs.string, codecs.i32);

  roundTrips("map<string, i32>", strI32, [
    new Map(),
    new Map([["a", 1]]),
    new Map([
      ["a", 1],
      ["b", 2],
      ["日", -3],
    ]),
  ]);
  roundTrips("map<u64, vec<string>>", codecs.map(codecs.u64, codecs.vec(codecs.string)), [
    new Map<bigint, string[]>([
      [U64_MAX, ["x"]],
      [0n, []],
      [BEYOND_SAFE, ["y", "z"]],
    ]),
  ]);
  roundTrips("map<uuid, todo>", codecs.map(codecs.uuid, todoCodec), [
    new Map([
      ["123e4567-e89b-12d3-a456-426614174000", { id: "123e4567-e89b-12d3-a456-426614174000", title: "a", done: true }],
      ["00000000-0000-0000-0000-000000000001", { id: "00000000-0000-0000-0000-000000000001", title: "b", done: false }],
    ]),
  ]);

  it("matches the contract vector: entries sorted by encoded key", () => {
    const m = new Map([
      ["b", 2],
      ["a", 1],
    ]);
    expect(toHex(encodeValue(strI32, m))).toBe("02000000010000006101000000010000006202000000");
  });

  it("produces identical bytes for every insertion order", () => {
    const entries: [string, number][] = [
      ["delta", 4],
      ["alpha", 1],
      ["charlie", 3],
      ["bravo", 2],
      ["echo", 5],
      ["x", 6],
      ["", 7],
    ];
    const reference = toHex(encodeValue(strI32, new Map(entries)));
    const rand = prng(41);
    for (let i = 0; i < 50; i++) {
      const shuffled = [...entries];
      for (let j = shuffled.length - 1; j > 0; j--) {
        const k = randInt(rand, j + 1);
        [shuffled[j], shuffled[k]] = [shuffled[k] as [string, number], shuffled[j] as [string, number]];
      }
      expect(toHex(encodeValue(strI32, new Map(shuffled)))).toBe(reference);
    }
  });

  it("sorts by the encoded key bytes, not by natural order", () => {
    // A string key starts with its little-endian u32 length, so "b" (01 00 00 00 ..) sorts before "aa" (02 00 00 00 ..).
    const s = encodeValue(codecs.map(codecs.string, codecs.u8), new Map([["aa", 1], ["b", 2]]));
    expect(toHex(s)).toBe("02000000" + "01000000" + "62" + "02" + "02000000" + "6161" + "01");
    // Little-endian u32 keys: 256 encodes as 00 01 00 00 and therefore sorts before 1 (01 00 00 00).
    const n = encodeValue(codecs.map(codecs.u32, codecs.u8), new Map([[1, 10], [256, 20]]));
    expect(toHex(n)).toBe("02000000" + "00010000" + "14" + "01000000" + "0a");
  });

  it("orders a key that is a prefix of another first", () => {
    const c = codecs.map(codecs.bytes, codecs.u8);
    const short = new Uint8Array([1]);
    const long = new Uint8Array([1, 2]);
    // Encoded: 01000000 01 vs 02000000 0102: the length prefix decides, so `short` is first either way.
    const a = toHex(encodeValue(c, new Map([[long, 2], [short, 1]])));
    const b = toHex(encodeValue(c, new Map([[short, 1], [long, 2]])));
    expect(a).toBe(b);
  });

  it("sorts a large map that arrives in reverse order", () => {
    const entries: [number, number][] = [];
    for (let i = 999; i >= 0; i--) entries.push([i, i * 2]);
    const c = codecs.map(codecs.u32, codecs.u32);
    const bytes = encodeValue(c, new Map(entries));
    const decoded = decodeValue(c, bytes);
    expect(decoded.size).toBe(1000);
    // Keys come back in encoded-byte order: strictly ascending by little-endian bytes.
    const keys = [...decoded.keys()];
    for (let i = 1; i < keys.length; i++) {
      const prev = new Uint8Array(4);
      const cur = new Uint8Array(4);
      new DataView(prev.buffer).setUint32(0, keys[i - 1] as number, true);
      new DataView(cur.buffer).setUint32(0, keys[i] as number, true);
      expect(Buffer.compare(prev, cur)).toBeLessThan(0);
    }
    expect(toHex(encodeValue(c, new Map(entries.slice().reverse())))).toBe(toHex(bytes));
  });

  it("keeps a map that is already sorted untouched", () => {
    const c = codecs.map(codecs.u8, codecs.string);
    const bytes = encodeValue(c, new Map([[1, "a"], [2, "bb"], [3, "ccc"]]));
    expect(toHex(bytes)).toBe("03000000" + "01" + "01000000" + "61" + "02" + "02000000" + "6262" + "03" + "03000000" + "636363");
  });

  it("encodes onto a writer that already holds data", () => {
    const c = codecs.map(codecs.u8, codecs.u8);
    const w = new UndraWriter();
    w.writeU32(0xdeadbeef);
    c.encode(w, new Map([[9, 1], [3, 2], [7, 3]]));
    w.writeU8(0xff);
    expect(toHex(w.finish())).toBe("efbeadde" + "03000000" + "0302" + "0703" + "0901" + "ff");
  });

  it("rejects two keys that encode to the same bytes when encoding", () => {
    const c = codecs.map(codecs.bytes, codecs.u8);
    const a = new Uint8Array([1, 2]);
    const b = new Uint8Array([1, 2]);
    expectWireError(() => encodeValue(c, new Map([[a, 1], [new Uint8Array([9]), 5], [b, 2]])), "duplicate_key");
    expectWireError(() => encodeValue(c, new Map([[a, 1], [b, 2]])), "duplicate_key");
  });

  it("rejects a duplicate key when decoding, reporting the offset of the second", () => {
    expectWireError(
      () => decodeValue(strI32, fromHex("02000000 01000000 61 01000000 01000000 61 02000000")),
      "duplicate_key",
      { at: 13 },
    );
  });

  it("accepts unsorted input when decoding and preserves wire order", () => {
    const decoded = decodeValue(strI32, fromHex("02000000 01000000 62 02000000 01000000 61 01000000"));
    expect([...decoded.entries()]).toEqual([
      ["b", 2],
      ["a", 1],
    ]);
  });

  it("rejects a count larger than the remaining input", () => {
    expectWireError(() => decodeValue(strI32, fromHex("09000000")), "length_too_large", { len: 9 });
  });
});

describe("result", () => {
  const c = codecs.result(codecs.i32, codecs.string);
  roundTrips("result<i32, string>", c, [{ ok: 5 }, { ok: -1 }, { err: "bad" }, { err: "" }, { err: "\u{1f30a}" }]);

  it("uses tag 0 for Ok and 1 for Err", () => {
    expect(toHex(encodeValue(c, { ok: 5 }))).toBe("0005000000");
    expect(toHex(encodeValue(c, { err: "bad" }))).toBe("0103000000626164");
  });

  it("supports a unit Ok", () => {
    const unitOk = codecs.result(codecs.unit, codecs.string);
    expect(toHex(encodeValue(unitOk, { ok: undefined }))).toBe("00");
    const decoded = decodeValue(unitOk, fromHex("00"));
    expect("ok" in decoded).toBe(true);
    expect("err" in decoded).toBe(false);
  });

  it("nests", () => {
    const nested = codecs.result(codecs.option(codecs.vec(codecs.u8)), codecs.result(codecs.bool, codecs.string));
    const values: WireResult<number[] | null, WireResult<boolean, string>>[] = [
      { ok: null },
      { ok: [1, 2] },
      { err: { ok: true } },
      { err: { err: "no" } },
    ];
    for (const v of values) expect(decodeValue(nested, encodeValue(nested, v))).toEqual(v);
  });

  it("rejects a tag other than 0 or 1", () => {
    expectWireError(() => decodeValue(c, fromHex("02 05000000")), "invalid_tag", { tag: 2, at: 0, ty: "Result" });
  });
});

describe("records and enums built from codecs", () => {
  roundTrips("record Todo", todoCodec, [
    { id: "123e4567-e89b-12d3-a456-426614174000", title: "Milk", done: false },
    { id: "00000000-0000-0000-0000-000000000000", title: "", done: true },
    { id: "ffffffff-ffff-ffff-ffff-ffffffffffff", title: "\u{1f30a} café", done: true },
  ]);
  roundTrips("enum Filter", filterCodec, ["all", "active", "done"] as Filter[]);
  roundTrips("enum Shape", shapeCodec, [
    { kind: "circle", radius: 1.5 },
    { kind: "rect", w: 2, h: 3 },
  ] as Shape[]);

  it("rejects an unknown variant index", () => {
    expectWireError(() => decodeValue(filterCodec, fromHex("0300")), "invalid_tag", { tag: 3, ty: "Filter" });
  });
});

/** The type of every member of `codecs` as the table in `wire/codec.ts` documents it: assigning the namespace to it is the compile-time pin. */
interface CodecsShape {
  readonly bool: Codec<boolean>;
  readonly u8: Codec<number>;
  readonly i8: Codec<number>;
  readonly u16: Codec<number>;
  readonly i16: Codec<number>;
  readonly u32: Codec<number>;
  readonly i32: Codec<number>;
  readonly u64: Codec<bigint>;
  readonly i64: Codec<bigint>;
  readonly u64Number: Codec<number>;
  readonly i64Number: Codec<number>;
  readonly f32: Codec<number>;
  readonly f64: Codec<number>;
  readonly unit: Codec<void>;
  readonly string: Codec<string>;
  readonly bytes: Codec<Uint8Array>;
  readonly duration: Codec<number>;
  readonly timestamp: Codec<number>;
  readonly uuid: Codec<string>;
  readonly handle: Codec<bigint>;
  option<T>(inner: Codec<T>): Codec<T | null>;
  vec<T>(item: Codec<T>): Codec<T[]>;
  map<K, V>(key: Codec<K>, value: Codec<V>): Codec<Map<K, V>>;
  result<T, E>(ok: Codec<T>, err: Codec<E>): Codec<WireResult<T, E>>;
}

describe("codecs namespace and helpers", () => {
  // The 24 names of SPEC 10.4 / `codecs`' table: generated code and hand-written code name them as `codecs.<name>` (ADR-057, D9).
  const NAMES = [
    "bool", "u8", "i8", "u16", "i16", "u32", "i32", "u64", "i64", "u64Number", "i64Number", "f32", "f64",
    "unit", "string", "bytes", "duration", "timestamp", "uuid", "handle", "option", "vec", "map", "result",
  ] as const;

  it("exposes exactly the 24 documented codecs, each a codec or a combinator, with the documented types", () => {
    const typed: CodecsShape = codecs;
    expect(typed).toBe(codecs);
    expect(Object.keys(codecs).sort()).toEqual([...NAMES].sort());
    for (const name of NAMES) expect(codecs[name], name).toBeDefined();
    for (const name of ["option", "vec", "map", "result"] as const) expect(typeof codecs[name], name).toBe("function");
    for (const name of NAMES.filter((n) => !["option", "vec", "map", "result"].includes(n))) {
      const codec = codecs[name as "bool"] as Codec<unknown>;
      expect([typeof codec.encode, typeof codec.decode], name).toEqual(["function", "function"]);
    }
  });

  it("cannot be reassigned (a module namespace, not a plain object: ADR-057 D9; its other module-namespace properties are checked on the built package, test/dist-flavour.test.ts)", () => {
    const view = codecs as unknown as Record<string, unknown>;
    expect(() => {
      view["u8"] = codecs.u16;
    }).toThrow(TypeError);
    expect(codecs.u8.encode).toBeTypeOf("function");
  });

  it("is what `export *` hands out: the wire barrel and the package root name the same codecs", async () => {
    const [wire, root] = await Promise.all([import("../src/wire/index.js"), import("../src/index.js")]);
    expect(wire.codecs).toBe(codecs);
    expect(root.codecs).toBe(codecs);
  });

  it("decodeValue requires the input to be consumed exactly", () => {
    expectWireError(() => decodeValue(codecs.u8, fromHex("0102")), "trailing_bytes", { count: 1 });
  });

  it("codecs compose over one shared writer and reader", () => {
    const w = new UndraWriter();
    codecs.u8.encode(w, 1);
    codecs.string.encode(w, "two");
    codecs.vec(codecs.u16).encode(w, [3, 4]);
    const r = new UndraReader(w.finish());
    expect(codecs.u8.decode(r)).toBe(1);
    expect(codecs.string.decode(r)).toBe("two");
    expect(codecs.vec(codecs.u16).decode(r)).toEqual([3, 4]);
    r.finish();
  });
});
