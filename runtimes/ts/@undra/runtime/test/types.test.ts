import { describe, expect, it } from "vitest";
import {
  NULL_HANDLE,
  decodeUuid,
  durationFromNanos,
  durationToNanos,
  encodeUuid,
  handleGeneration,
  handleIndex,
  joinHandle,
  makeHandle,
  splitHandle,
  timestampFromDate,
  timestampToDate,
} from "../src/wire/types.js";
import { expectWireError, fromHex, prng, randInt, randomBytes, toHex } from "./helpers.js";

describe("Handle", () => {
  it("packs index into the low 24 and generation into the high 40 bits (ADR-040)", () => {
    const h = makeHandle(1, 1);
    expect(h).toBe(16777217n);
    expect(handleIndex(h)).toBe(1);
    expect(handleGeneration(h)).toBe(1);
    expect(makeHandle(0xbeef, 0x12_3456_789a)).toBe(0x12_3456_789a_00beefn);
  });

  it("round-trips the extremes", () => {
    for (const [index, generation] of [
      [0, 0],
      [0, 1],
      [0xffffff, 0],
      [0, 0xff_ffff_ffff],
      [0xffffff, 0xff_ffff_ffff],
      [7, 9],
    ] as const) {
      const h = makeHandle(index, generation);
      expect(handleIndex(h)).toBe(index);
      expect(handleGeneration(h)).toBe(generation);
    }
    expect(makeHandle(0xffffff, 0xff_ffff_ffff)).toBe(2n ** 64n - 1n);
  });

  it("the null handle is zero", () => {
    expect(NULL_HANDLE).toBe(0n);
    expect(handleIndex(NULL_HANDLE)).toBe(0);
    expect(handleGeneration(NULL_HANDLE)).toBe(0);
    expect(makeHandle(0, 0)).toBe(NULL_HANDLE);
  });

  it("rejects an index outside 24 bits or a generation outside 40 bits", () => {
    expect(() => makeHandle(-1, 1)).toThrow(RangeError);
    expect(() => makeHandle(2 ** 24, 1)).toThrow(RangeError);
    expect(() => makeHandle(1, -1)).toThrow(RangeError);
    expect(() => makeHandle(1, 2 ** 40)).toThrow(RangeError);
    expect(() => makeHandle(1.5, 1)).toThrow(RangeError);
    expect(() => makeHandle(Number.NaN, 1)).toThrow(RangeError);
  });

  it("splits into unsigned halves for the wasm ABI", () => {
    expect(splitHandle(4294967297n)).toEqual({ lo: 1, hi: 1 });
    expect(splitHandle(0n)).toEqual({ lo: 0, hi: 0 });
    expect(splitHandle(2n ** 64n - 1n)).toEqual({ lo: 0xffffffff, hi: 0xffffffff });
    expect(splitHandle(0x80000000_ffffffffn)).toEqual({ lo: 0xffffffff, hi: 0x80000000 });
  });

  it("rejects a value that is not a u64", () => {
    expect(() => splitHandle(2n ** 64n)).toThrow(RangeError);
    expect(() => splitHandle(-1n)).toThrow(RangeError);
  });

  it("joins unsigned halves", () => {
    expect(joinHandle(1, 1)).toBe(4294967297n);
    expect(joinHandle(0xffffffff, 0xffffffff)).toBe(2n ** 64n - 1n);
  });

  it("joins the signed i32 values that wasm hands to JS", () => {
    expect(joinHandle(-1, -1)).toBe(2n ** 64n - 1n);
    expect(joinHandle(-2147483648, 0)).toBe(0x80000000n);
    expect(joinHandle(0, -2147483648)).toBe(0x80000000_00000000n);
  });

  it("split then join is the identity", () => {
    const rand = prng(5);
    for (let i = 0; i < 500; i++) {
      const h = makeHandle(randInt(rand, 2 ** 24), randInt(rand, 2 ** 32) * 256 + randInt(rand, 256));
      const { lo, hi } = splitHandle(h);
      expect(joinHandle(lo, hi)).toBe(h);
      // Through the i32 representation wasm uses.
      expect(joinHandle(lo | 0, hi | 0)).toBe(h);
    }
  });
});

describe("Timestamp", () => {
  it("converts to and from Date", () => {
    const date = new Date("2024-09-30T00:00:00.000Z");
    expect(timestampFromDate(date)).toBe(1727654400000);
    expect(timestampToDate(1727654400000).toISOString()).toBe("2024-09-30T00:00:00.000Z");
  });

  it("handles the epoch, negative and extreme values", () => {
    expect(timestampFromDate(new Date(0))).toBe(0);
    expect(timestampToDate(-1).getTime()).toBe(-1);
    expect(timestampToDate(8.64e15).getTime()).toBe(8.64e15);
    expect(timestampToDate(-8.64e15).getTime()).toBe(-8.64e15);
  });

  it("rejects what a Date cannot hold", () => {
    expect(() => timestampFromDate(new Date(Number.NaN))).toThrow(RangeError);
    expect(() => timestampToDate(8.64e15 + 1)).toThrow(RangeError);
    expect(() => timestampToDate(Number.NaN)).toThrow(RangeError);
  });
});

describe("Duration", () => {
  it("converts whole milliseconds exactly", () => {
    expect(durationToNanos(0)).toBe(0n);
    expect(durationToNanos(1)).toBe(1_000_000n);
    expect(durationToNanos(1500)).toBe(1_500_000_000n);
    expect(durationToNanos(9_223_372_036_854)).toBe(9_223_372_036_854_000_000n);
    expect(durationFromNanos(1_500_000_000n)).toBe(1500);
    expect(durationFromNanos(0n)).toBe(0);
  });

  it("keeps sub-millisecond precision down to a nanosecond", () => {
    expect(durationToNanos(0.5)).toBe(500_000n);
    expect(durationToNanos(0.000001)).toBe(1n);
    expect(durationToNanos(1.000001)).toBe(1_000_001n);
    expect(durationFromNanos(1n)).toBe(0.000001);
    expect(durationFromNanos(1_000_001n)).toBeCloseTo(1.000001, 12);
  });

  it("round-trips nanosecond counts that a double can hold", () => {
    for (const ns of [0n, 1n, 999_999n, 1_000_000n, 123_456_789n, 86_400_000_000_000n]) {
      expect(durationToNanos(durationFromNanos(ns))).toBe(ns);
    }
  });

  it("converts the largest representable durations without losing the whole-millisecond part", () => {
    const max = 0x7fff_ffff_ffff_ffffn;
    expect(durationFromNanos(max)).toBeCloseTo(9_223_372_036_854.775, 2);
    expect(Math.trunc(durationFromNanos(max))).toBe(9_223_372_036_854);
  });

  it("rejects negative durations in both directions", () => {
    expectWireError(() => durationToNanos(-1), "negative_duration", { nanos: -1_000_000n });
    expectWireError(() => durationToNanos(-0.5), "negative_duration", { nanos: -500_000n });
    expectWireError(() => durationFromNanos(-1n), "negative_duration", { nanos: -1n });
    expectWireError(() => durationFromNanos(-(2n ** 63n)), "negative_duration");
  });

  it("treats -0 and sub-nanosecond negatives as zero", () => {
    expect(durationToNanos(-0)).toBe(0n);
    expect(durationToNanos(-1e-9)).toBe(0n);
  });

  it("rejects non-finite durations and durations beyond i64 nanoseconds", () => {
    expect(() => durationToNanos(Number.NaN)).toThrow(RangeError);
    expect(() => durationToNanos(Number.POSITIVE_INFINITY)).toThrow(RangeError);
    expect(() => durationToNanos(9_223_372_036_855)).toThrow(RangeError);
    expect(() => durationToNanos(1e300)).toThrow(RangeError);
  });
});

describe("Uuid", () => {
  const text = "123e4567-e89b-12d3-a456-426614174000";
  const raw = "123e4567e89b12d3a456426614174000";

  it("encodes to 16 raw bytes in RFC 4122 order", () => {
    expect(toHex(encodeUuid(text))).toBe(raw);
  });

  it("decodes to the canonical lowercase hyphenated form", () => {
    expect(decodeUuid(fromHex(raw))).toBe(text);
    expect(decodeUuid(fromHex("00000000000000000000000000000000"))).toBe("00000000-0000-0000-0000-000000000000");
    expect(decodeUuid(fromHex("ffffffffffffffffffffffffffffffff"))).toBe("ffffffff-ffff-ffff-ffff-ffffffffffff");
    expect(decodeUuid(fromHex("0123456789abcdef0123456789abcdef"))).toBe("01234567-89ab-cdef-0123-456789abcdef");
  });

  it("accepts upper case on input but always produces lower case", () => {
    const upper = "123E4567-E89B-12D3-A456-426614174000";
    expect(toHex(encodeUuid(upper))).toBe(raw);
    expect(decodeUuid(encodeUuid(upper))).toBe(text);
  });

  it("round-trips random UUIDs", () => {
    const rand = prng(9);
    for (let i = 0; i < 1000; i++) {
      const bytes = randomBytes(rand, 16);
      const s = decodeUuid(bytes);
      expect(s).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
      expect(toHex(encodeUuid(s))).toBe(toHex(bytes));
    }
  });

  it("writes into a caller buffer at an offset and touches nothing else", () => {
    const buf = new Uint8Array(20).fill(0xee);
    const out = encodeUuid(text, buf, 2);
    expect(out).toBe(buf);
    expect(toHex(buf)).toBe(`eeee${raw}eeee`);
  });

  it("decodes from an offset in a larger buffer", () => {
    const buf = fromHex(`aabb${raw}ccdd`);
    expect(decodeUuid(buf, 2)).toBe(text);
    const view = buf.subarray(1);
    expect(decodeUuid(view, 1)).toBe(text);
  });

  it("rejects malformed strings", () => {
    for (const bad of [
      "",
      "123e4567",
      `${text}0`,
      text.slice(1),
      "123e4567e89b12d3a456426614174000    ",
      "123e4567-e89b-12d3-a456-42661417400g",
      "123e4567-e89b-12d3-a456-4266141740 0",
      "123e4567_e89b-12d3-a456-426614174000",
      "{123e4567-e89b-12d3-a456-42661417400}",
      "123e4567-e89b-12d3-a456-42661417400é",
      "-23e4567-e89b-12d3-a456-426614174000",
    ]) {
      expect(() => encodeUuid(bad), bad).toThrow(RangeError);
    }
  });

  it("says the same thing for every way a UUID can be wrong: it names the input and the shape (ADR-057: one message)", () => {
    const messages = new Set<string>();
    for (const bad of ["", "123e4567", `${text}0`, "123e4567-e89b-12d3-a456-42661417400g", "123e4567_e89b-12d3-a456-426614174000"]) {
      try {
        encodeUuid(bad);
        expect.unreachable(bad);
      } catch (error) {
        expect(error).toBeInstanceOf(RangeError);
        expect((error as RangeError).message).toContain(`"${bad}"`);
        messages.add((error as RangeError).message.replace(`"${bad}"`, "<input>"));
      }
    }
    expect([...messages], "one sentence, whatever was wrong").toHaveLength(1);
    expect(() => encodeUuid(text, new Uint8Array(20), 5)).toThrow(/room for 16 bytes at offset 5 of 20/);
  });

  it("decodes every byte value at every position as two lowercase hex digits", () => {
    for (let position = 0; position < 16; position++) {
      for (let value = 0; value < 256; value++) {
        const bytes = new Uint8Array(16);
        bytes[position] = value;
        const hex = toHex(bytes);
        expect(decodeUuid(bytes)).toBe(`${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`);
      }
    }
  });

  it("rejects a destination that is too small", () => {
    expect(() => encodeUuid(text, new Uint8Array(15))).toThrow(RangeError);
    expect(() => encodeUuid(text, new Uint8Array(20), 5)).toThrow(RangeError);
    expect(() => encodeUuid(text, new Uint8Array(20), -1)).toThrow(RangeError);
  });

  it("rejects a source with fewer than 16 bytes", () => {
    expect(() => decodeUuid(new Uint8Array(15))).toThrow(RangeError);
    expect(() => decodeUuid(new Uint8Array(20), 5)).toThrow(RangeError);
    expect(() => decodeUuid(new Uint8Array(20), -1)).toThrow(RangeError);
  });
});
