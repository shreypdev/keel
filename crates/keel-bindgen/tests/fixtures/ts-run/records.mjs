// Execution test of the `records` golden case. Usage: node records.mjs <package> [js-number].

import assert from "node:assert/strict";

import { bytes, hex, setup } from "./lib.mjs";

const { rt, types } = await setup(process.argv[2]);
const { encodeValue, decodeValue } = rt;
const jsNumber = process.argv[3] === "js-number";

// Every fixed-width number type, little-endian, and a duration in nanoseconds.
const wide = (big) => (jsNumber ? Number(big) : big);
const numbers = {
  a: -1,
  b: -2,
  c: -3,
  d: wide(-4n),
  e: 255,
  f: 65535,
  g: 4294967295,
  h: wide(jsNumber ? 4503599627370496n : 18446744073709551615n),
  i: 1.5,
  j: 2.5,
  elapsed: 1500,
};
const numbersText =
  "ff" + "feff" + "fdffffff" + "fcffffffffffffff" +
  "ff" + "ffff" + "ffffffff" + (jsNumber ? "0000000000001000" : "ffffffffffffffff") +
  "0000c03f" + "0000000000000440" +
  "002f685900000000";
assert.equal(hex(encodeValue(types.NumbersCodec, numbers)), numbersText);
const decodedNumbers = decodeValue(types.NumbersCodec, bytes(numbersText));
assert.deepEqual(decodedNumbers, numbers);
assert.equal(typeof decodedNumbers.d, jsNumber ? "number" : "bigint");
assert.throws(() => decodeValue(types.NumbersCodec, bytes(numbersText.slice(0, -2))), (e) => e.code === "unexpected_eof");
assert.throws(
  () => decodeValue(types.NumbersCodec, bytes(numbersText.replace("002f685900000000", "ffffffffffffffff"))),
  (e) => e.code === "negative_duration",
);
if (jsNumber) {
  // Beyond 2^53 - 1 a `number` is not exact: decoding refuses.
  const unsafe = numbersText.replace("0000000000001000", "0000000000002000");
  assert.throws(() => decodeValue(types.NumbersCodec, bytes(unsafe)), (e) => e.code === "unsafe_integer");
}

// Containers, nested and optional.
const containers = {
  lines: ["a", null, ""],
  scores: new Map([["k", 7], ["j", -1]]),
  byId: new Map([["00000000-0000-0000-0000-000000000001", [{ id: "00000000-0000-0000-0000-000000000002", title: "t", done: false, tags: [], due: null, priority: "low" }]]]),
  blob: new Uint8Array([1, 2, 3]),
  maybeBlob: null,
  matrix: [[1.5], []],
  counts: new Map([[1, wide(2n)]]),
};
const roundTripped = decodeValue(types.ContainersCodec, encodeValue(types.ContainersCodec, containers));
assert.deepEqual(roundTripped, containers);
assert.equal(hex(encodeValue(types.ContainersCodec, { ...containers, lines: [], scores: new Map(), byId: new Map(), blob: new Uint8Array(0), matrix: [], counts: new Map() })), "00000000".repeat(3) + "00000000" + "00" + "00000000" + "00000000");

// Reserved words are fine as property names.
const keywords = { default: "d", in: 1, object: true, delete: false, new: true, className: "c" };
assert.deepEqual(decodeValue(types.KeywordsCodec, encodeValue(types.KeywordsCodec, keywords)), keywords);

// A record without fields is zero bytes.
assert.equal(encodeValue(types.EmptyCodec, {}).length, 0);
assert.deepEqual(decodeValue(types.EmptyCodec, new Uint8Array(0)), {});

// A recursive record.
const page = { items: [], next: "n", children: [{ items: [], next: null, children: [] }] };
assert.deepEqual(decodeValue(types.PageCodec, encodeValue(types.PageCodec, page)), page);

console.log("ok");
