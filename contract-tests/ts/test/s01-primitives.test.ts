import { expect, test } from "vitest";
import { durationToNanos } from "@keel/runtime";
import { type Primitives, echoPrimitives, ping } from "@playground/core";
import { boot } from "../src/harness.js";
import { step } from "../src/wait.js";

// S01 primitives round-trip: every primitive the wire has makes the round trip through the
// language's codec unchanged.

const ids = {
  typical: "12345678-9abc-def0-0102-030405060708",
};

/** The typical value of step 1. `span` is 1.500000123 s in milliseconds (the TypeScript `Duration`). */
function typical(): Primitives {
  return {
    flag: true,
    tiny: -8,
    small: -16000,
    int: -2_000_000_000,
    long: -9_000_000_000_000_000_000n,
    byte: 255,
    word: 65535,
    dword: 4_000_000_000,
    qword: 18_000_000_000_000_000_000n,
    single: 1.5,
    double: -2.25e100,
    text: "héllo, wörld ✓",
    blob: Uint8Array.of(0, 1, 2, 254, 255),
    span: 1500.000123,
    at: 1_700_000_000_123,
    id: ids.typical,
  };
}

/** The extremes of step 2: every field at the edge of its type. */
function extremes(): Primitives {
  return {
    flag: false,
    tiny: -128,
    small: -32768,
    int: -2_147_483_648,
    long: -9_223_372_036_854_775_808n,
    byte: 0,
    word: 0,
    dword: 4_294_967_295,
    qword: 18_446_744_073_709_551_615n,
    single: -0,
    double: Number.POSITIVE_INFINITY,
    text: "",
    blob: new Uint8Array(0),
    span: 0,
    at: -8_640_000_000_000_000,
    id: "00000000-0000-0000-0000-000000000000",
  };
}

/** The same fields at the other edge, with NaN, a long text and a blob bigger than the runtime's scratch buffer. */
function highs(): Primitives {
  return {
    flag: true,
    tiny: 127,
    small: 32767,
    int: 2_147_483_647,
    long: 9_223_372_036_854_775_807n,
    byte: 255,
    word: 65535,
    dword: 0,
    qword: 0n,
    single: 3.4028234663852886e38,
    double: Number.NaN,
    text: "ü".repeat(10_000),
    blob: Uint8Array.from({ length: 65_536 }, (_, i) => i % 251),
    span: 9_000_000_000,
    at: 8_640_000_000_000_000,
    id: "ffffffff-ffff-ffff-ffff-ffffffffffff",
  };
}

/** Field by field, with the two comparisons `toEqual` gets wrong: NaN is NaN, and -0 is not +0. */
function expectSame(got: Primitives, sent: Primitives): void {
  const { blob: gotBlob, single: gotSingle, double: gotDouble, span: gotSpan, ...gotRest } = got;
  const { blob: sentBlob, single: sentSingle, double: sentDouble, span: sentSpan, ...sentRest } = sent;
  expect(gotRest).toEqual(sentRest);
  expect(Array.from(gotBlob)).toEqual(Array.from(sentBlob));
  expect(Object.is(gotSingle, sentSingle), `single ${String(gotSingle)} vs ${String(sentSingle)}`).toBe(true);
  expect(Object.is(gotDouble, sentDouble), `double ${String(gotDouble)} vs ${String(sentDouble)}`).toBe(true);
  // The duration is whole nanoseconds on the wire: compare those, not two floating-point milliseconds.
  expect(durationToNanos(gotSpan)).toBe(durationToNanos(sentSpan));
}

test("S01 primitives round-trip", async () => {
  const { core } = await boot();

  await step("1. the typical value", async () => {
    const sent = typical();
    const got = await echoPrimitives(sent, core);
    expectSame(got, sent);
    expect(got.long).toBe(-9_000_000_000_000_000_000n);
    expect(got.qword).toBe(18_000_000_000_000_000_000n);
    expect(got.span).toBeCloseTo(1500.000123, 9);
  });

  await step("2a. the low extremes", async () => {
    const sent = extremes();
    const got = await echoPrimitives(sent, core);
    expectSame(got, sent);
    expect(Object.is(got.single, -0)).toBe(true);
    expect(got.double).toBe(Number.POSITIVE_INFINITY);
    expect(got.long).toBe(-9_223_372_036_854_775_808n);
    expect(got.qword).toBe(18_446_744_073_709_551_615n);
  });

  await step("2b. the high extremes, NaN, 10,000 characters and 65,536 bytes", async () => {
    const sent = highs();
    const got = await echoPrimitives(sent, core);
    expectSame(got, sent);
    expect(got.long).toBe(9_223_372_036_854_775_807n);
    expect(Number.isNaN(got.double)).toBe(true);
    expect(got.text).toHaveLength(10_000);
    expect(got.blob).toHaveLength(65_536);
  });

  await step("3. ping() completes", async () => {
    await expect(ping(core)).resolves.toBeUndefined();
  });

  await step("4. bytes, strings and uuids are not aliased", async () => {
    const sent = typical();
    const first = await echoPrimitives(sent, core);
    const second = await echoPrimitives(sent, core);
    // A returned blob is its own array over its own buffer: not the sent one, not a view of wasm memory...
    expect(first.blob).not.toBe(sent.blob);
    expect(first.blob.buffer).not.toBe(sent.blob.buffer);
    expect(first.blob).not.toBe(second.blob);
    expect(first.blob.buffer).not.toBe(second.blob.buffer);
    // ...so mutating it changes nothing that was sent or returned before or after...
    first.blob[0] = 99;
    expect(Array.from(sent.blob)).toEqual([0, 1, 2, 254, 255]);
    expect(Array.from(second.blob)).toEqual([0, 1, 2, 254, 255]);
    // ...and later calls (which reuse the runtime's scratch buffer and the core's memory) do not reach back into it.
    const other = { ...typical(), blob: Uint8Array.of(9, 9, 9, 9, 9, 9, 9, 9) };
    await echoPrimitives(other, core);
    expect(Array.from(second.blob)).toEqual([0, 1, 2, 254, 255]);
    // Changing what was sent after the call returned does not reach the answer either.
    sent.blob[1] = 77;
    expect(Array.from(second.blob)).toEqual([0, 1, 2, 254, 255]);
    // Strings and uuids are immutable values in TypeScript: equal, and nothing to alias.
    expect(second.text).toBe("héllo, wörld ✓");
    expect(second.id).toBe(ids.typical);
  });
});
