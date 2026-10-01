// Execution test of the `enums` golden case. Usage: node enums.mjs <package>.

import assert from "node:assert/strict";

import { bytes, hex, setup } from "./lib.mjs";

const { rt, types } = await setup(process.argv[2]);
const { encodeValue, decodeValue } = rt;

// Unit enums map indexes to camelCase literals; indexes need not be dense.
assert.equal(hex(encodeValue(types.NetKindCodec, "default")), "0300");
assert.equal(decodeValue(types.NetKindCodec, bytes("0200")), "none");
assert.equal(hex(encodeValue(types.SparseCodec, "second")), "0700");
assert.equal(decodeValue(types.SparseCodec, bytes("0100")), "first");
assert.throws(() => decodeValue(types.SparseCodec, bytes("0000")), (e) => e.code === "invalid_tag" && e.detail.tag === 0);
assert.throws(() => decodeValue(types.SparseCodec, bytes("0200")), (e) => e.code === "invalid_tag");

// Data enums: named fields, tuple fields, a field called `kind`, unit variants.
const shapes = [
  [{ kind: "circle", radius: 1.5 }, "0000" + "000000000000f83f"],
  [{ kind: "rect", value0: 1.5, value1: 1.5 }, "0100" + "000000000000f83f" + "000000000000f83f"],
  [{ kind: "labelled", label: "a", kind_: -2, inner: "done" }, "0200" + "01000000" + "61" + "feffffff" + "01" + "0200"],
  [{ kind: "labelled", label: "", kind_: 0, inner: null }, "0200" + "00000000" + "00000000" + "00"],
  [{ kind: "single", value: "s" }, "0300" + "01000000" + "73"],
  [{ kind: "empty" }, "0400"],
];
for (const [value, text] of shapes) {
  assert.equal(hex(encodeValue(types.ShapeCodec, value)), text, JSON.stringify(value));
  assert.deepEqual(decodeValue(types.ShapeCodec, bytes(text)), value);
}
assert.throws(() => decodeValue(types.ShapeCodec, bytes("0500")), (e) => e.code === "invalid_tag");

// A recursive enum whose variants are named like standard types.
const value = {
  kind: "list",
  value: [
    { kind: "int", value: 5n },
    { kind: "string", value: "x" },
    { kind: "bool", value: true },
    { kind: "shape", value: { kind: "empty" } },
    { kind: "filter", value: "done" },
    { kind: "list", value: [] },
    { kind: "null" },
  ],
};
const text =
  "0300" + "07000000" +
  "0100" + "0500000000000000" +
  "0000" + "01000000" + "78" +
  "0200" + "01" +
  "0400" + "0400" +
  "0500" + "0200" +
  "0300" + "00000000" +
  "0600";
assert.equal(hex(encodeValue(types.ValueCodec, value)), text);
assert.deepEqual(decodeValue(types.ValueCodec, bytes(text)), value);

console.log("ok");
