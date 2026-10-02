import { describe, expect, it } from "vitest";
import { diffLines, diffValues, equal, patchLines, patchSummary } from "../src/diff.js";
import { formatValue, pretty } from "../src/format.js";
import type { PatchOp } from "../src/value.js";

describe("equality", () => {
  it("is structural, over every shape a value takes", () => {
    expect(equal({ a: [1, { b: "x" }] }, { a: [1, { b: "x" }] })).toBe(true);
    expect(equal({ a: 1 }, { a: 1, b: 2 })).toBe(false);
    expect(equal([1, 2], [1, 2, 3])).toBe(false);
    expect(equal(Uint8Array.of(1, 2), Uint8Array.of(1, 2))).toBe(true);
    expect(equal(Uint8Array.of(1, 2), Uint8Array.of(1, 3))).toBe(false);
    expect(equal(null, undefined)).toBe(false);
    expect(equal(1, "1")).toBe(false);
    expect(equal(2n ** 60n, 2n ** 60n)).toBe(true);
  });
});

describe("what changed between two values", () => {
  it("is nothing for equal values and a before/after for a scalar", () => {
    expect(diffValues(3, 3)).toEqual({ kind: "same" });
    expect(diffLines(diffValues(3, 4))).toEqual(["3  →  4"]);
    expect(diffLines(diffValues(undefined, "x"))).toEqual(['·  →  "x"']);
  });

  it("names only the fields of a record that changed", () => {
    const d = diffValues({ id: 1, title: "a", done: false }, { id: 1, title: "a", done: true });
    expect(d).toMatchObject({ kind: "fields", changes: [{ key: "done" }] });
    expect(diffLines(d)).toEqual(["done: false  →  true"]);
  });

  it("compares a variant's fields when the variant is the same, and replaces it when it is not", () => {
    expect(diffLines(diffValues({ $: "Range", from: 1, to: 2 }, { $: "Range", from: 1, to: 9 }))).toEqual(["to: 2  →  9"]);
    expect(diffLines(diffValues({ $: "All" }, { $: "Tag", "0": "x" }))).toEqual(['All  →  Tag("x")']);
  });

  it("compares short lists item by item and long ones by length", () => {
    expect(diffLines(diffValues([1, 2, 3], [1, 5, 3, 4]))).toEqual(["3 → 4 items", "[1] 2  →  5", "[3] ·  →  4"]);
    const long = Array.from({ length: 500 }, (_, i) => i);
    expect(diffLines(diffValues(long, [...long, 1]))).toEqual(["500 → 501 items"]);
  });

  it("shows at most a dozen changed items", () => {
    const a = Array.from({ length: 40 }, () => 0);
    const b = Array.from({ length: 40 }, () => 1);
    const lines = diffLines(diffValues(a, b));
    expect(lines.length).toBe(1 + 12 + 1);
    expect(lines.at(-1)).toBe("… 28 more");
  });
});

describe("keyed patches as text", () => {
  const ops: PatchOp[] = [
    { op: "insert", index: 2, item: { id: 3 } },
    { op: "remove", index: 0 },
    { op: "update", index: 1, item: { id: 9 } },
    { op: "move", from: 4, to: 0 },
    { op: "clear" },
  ];
  it("lists the operations and counts them", () => {
    expect(patchLines(ops)).toEqual(["+ [2] {id: 3}", "− [0]", "~ [1] {id: 9}", "↷ [4] → [0]", "clear"]);
    expect(patchSummary(ops)).toBe("clear +1 −1 ~1 ↷1");
    expect(patchSummary([{ op: "insert", index: 0, item: 1 }, { op: "insert", index: 0, item: 2 }])).toBe("+2");
  });
  it("cuts a long patch", () => {
    const many: PatchOp[] = Array.from({ length: 30 }, (_, i) => ({ op: "remove", index: i }));
    expect(patchLines(many).length).toBe(17);
    expect(patchLines(many).at(-1)).toBe("… 14 more operations");
  });
});

describe("one-line and indented views of a value", () => {
  it("shows the shapes compactly", () => {
    expect(formatValue({ id: 1, title: "x" })).toBe('{id: 1, title: "x"}');
    expect(formatValue({ $: "Ok", value: 3 })).toBe("Ok(value: 3)");
    expect(formatValue({ $: "Tag", "0": "x" })).toBe('Tag("x")');
    expect(formatValue({ $ts: 0 })).toBe("1970-01-01T00:00:00.000Z");
    expect(formatValue({ $dur: 1_500_000 })).toBe("1.5 ms");
    expect(formatValue(Uint8Array.of(1, 2, 255))).toBe("bytes(3) 0102ff");
    expect(formatValue(2n ** 70n)).toBe(`${2n ** 70n}n`);
    expect(formatValue({ $map: [["a", 1]] })).toBe('{"a": 1}');
    expect(formatValue(Array.from({ length: 100 }, (_, i) => i), 30)).toMatch(/…$/);
    expect(formatValue(undefined)).toBe("·");
  });
  it("indents a nested value", () => {
    expect(pretty({ a: { b: [1, { c: 2 }] } })).toBe("{\n  a: {\n    b: [\n      1,\n      {\n        c: 2\n      }\n    ]\n  }\n}");
    expect(pretty([1, 2, 3])).toBe("[1, 2, 3]");
  });
});
