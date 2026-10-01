import { describe, expect, it } from "vitest";
import { WireError } from "@undra/runtime/wire";
import { applyPatch, decodeArgs, decodePatchOps, decodeReplyBody, decodeValue, PatchMismatch, ValueError } from "../src/value.js";
import { bytes, index, listBytes, patchBytes, todoBytes } from "./helpers.js";

const idx = index();

describe("decoding values by schema type", () => {
  it("reads scalars, options and strings", () => {
    expect(decodeValue(idx, { kind: "i32" }, bytes((w) => w.writeI32(-5)))).toBe(-5);
    expect(decodeValue(idx, { kind: "string" }, bytes((w) => w.writeStr("héllo")))).toBe("héllo");
    expect(decodeValue(idx, { kind: "bool" }, Uint8Array.of(1))).toBe(true);
    expect(decodeValue(idx, { kind: "option", of: { kind: "u8" } }, Uint8Array.of(0))).toBeNull();
    expect(decodeValue(idx, { kind: "option", of: { kind: "u8" } }, Uint8Array.of(1, 9))).toBe(9);
    expect(decodeValue(idx, { kind: "unit" }, new Uint8Array(0))).toBeNull();
  });

  it("keeps a 64-bit integer exact: a number when it is safe, a bigint when it is not", () => {
    expect(decodeValue(idx, { kind: "u64" }, bytes((w) => w.writeU64(42n)))).toBe(42);
    expect(decodeValue(idx, { kind: "u64" }, bytes((w) => w.writeU64(2n ** 63n)))).toBe(2n ** 63n);
    expect(decodeValue(idx, { kind: "i64" }, bytes((w) => w.writeI64(-(2n ** 62n))))).toBe(-(2n ** 62n));
  });

  it("reads a record in declaration order", () => {
    expect(decodeValue(idx, { kind: "named", of: "Todo" }, todoBytes(3, "milk", true))).toEqual({ id: 3, title: "milk", done: true });
  });

  it("reads enum variants: unit, tuple and named fields", () => {
    const filter = { kind: "named", of: "Filter" } as const;
    expect(decodeValue(idx, filter, Uint8Array.of(0, 0))).toEqual({ $: "All" });
    expect(decodeValue(idx, filter, bytes((w) => { w.writeU16(1); w.writeStr("home"); }))).toEqual({ $: "Tag", "0": "home" });
    expect(decodeValue(idx, filter, bytes((w) => { w.writeU16(2); w.writeI32(1); w.writeI32(9); }))).toEqual({ $: "Range", from: 1, to: 9 });
    expect(() => decodeValue(idx, filter, Uint8Array.of(9, 0))).toThrow(WireError);
  });

  it("reads lists, maps, results, timestamps, durations and handles", () => {
    expect(decodeValue(idx, { kind: "vec", of: { kind: "named", of: "Todo" } }, listBytes([[1, "a"], [2, "b", true]]))).toEqual([
      { id: 1, title: "a", done: false },
      { id: 2, title: "b", done: true },
    ]);
    const map = bytes((w) => { w.writeLen(2); w.writeStr("a"); w.writeI32(1); w.writeStr("b"); w.writeI32(2); });
    expect(decodeValue(idx, { kind: "map", of: [{ kind: "string" }, { kind: "i32" }] }, map)).toEqual({ $map: [["a", 1], ["b", 2]] });
    const result = { kind: "result", of: [{ kind: "u16" }, { kind: "string" }] } as const;
    expect(decodeValue(idx, result, bytes((w) => { w.writeU8(0); w.writeU16(200); }))).toEqual({ $: "Ok", value: 200 });
    expect(decodeValue(idx, result, bytes((w) => { w.writeU8(1); w.writeStr("boom"); }))).toEqual({ $: "Err", value: "boom" });
    expect(decodeValue(idx, { kind: "timestamp" }, bytes((w) => w.writeI64(1_700_000_000_000n)))).toEqual({ $ts: 1_700_000_000_000 });
    expect(decodeValue(idx, { kind: "duration" }, bytes((w) => w.writeI64(1_500_000n)))).toEqual({ $dur: 1_500_000 });
    expect(decodeValue(idx, { kind: "named", of: "Counter" }, bytes((w) => w.writeU64(7n)))).toEqual({ $handle: 7n });
  });

  it("refuses what the schema cannot describe or the wire cannot hold", () => {
    expect(() => decodeValue(idx, { kind: "named", of: "Nothing" }, new Uint8Array(0))).toThrow(WireError);
    expect(() => decodeValue(idx, { kind: "i32" }, Uint8Array.of(1, 2))).toThrow(WireError);
    expect(() => decodeValue(idx, { kind: "i32" }, Uint8Array.of(1, 2, 3, 4, 5))).toThrow(WireError);
    expect(() => decodeValue(idx, { kind: "vec", of: { kind: "u8" } }, Uint8Array.of(255, 255, 255, 255))).toThrow(WireError);
    expect(() => decodeValue(idx, { kind: "option", of: { kind: "u8" } }, Uint8Array.of(2))).toThrow(WireError);
  });

  it("stops at a nesting depth instead of overflowing the stack", () => {
    let ty: import("../src/schema.js").TypeRef = { kind: "u8" };
    for (let i = 0; i < 80; i++) ty = { kind: "option", of: ty };
    expect(() => decodeValue(idx, ty, new Uint8Array(200).fill(1))).toThrow(ValueError);
  });
});

describe("arguments and replies of a port call", () => {
  const send = idx.portMethod(300, 301);
  it("reads the parameters in order, and the reply by status", () => {
    if (send === undefined) throw new Error("no method");
    expect(decodeArgs(idx, send.params, bytes((w) => { w.writeStr("https://x"); w.writeU8(3); }))).toEqual({ url: "https://x", retries: 3 });
    expect(decodeReplyBody(idx, send.returns, 0, bytes((w) => w.writeU16(204)))).toBe(204);
    expect(decodeReplyBody(idx, send.returns, 1, bytes((w) => w.writeStr("timeout")))).toBe("timeout");
    expect(decodeReplyBody(idx, send.returns, 2, new Uint8Array(0))).toBeUndefined();
    expect(() => decodeArgs(idx, send.params, Uint8Array.of(1))).toThrow(WireError);
  });
});

describe("keyed patches (SPEC 3.8)", () => {
  const list = [
    { id: 1, title: "a", done: false },
    { id: 2, title: "b", done: false },
    { id: 3, title: "c", done: false },
  ];
  const ops = (o: Parameters<typeof patchBytes>[0]) => decodePatchOps(idx, { kind: "named", of: "Todo" }, patchBytes(o));
  const titles = (l: readonly import("../src/value.js").Value[]) => l.map((x) => (x as { title: string }).title).join("");

  it("decodes every operation", () => {
    expect(ops([["insert", 1, 9, "x"], ["remove", 0], ["update", 2, 8, "y"], ["move", 0, 2], ["clear"]]).map((o) => o.op)).toEqual(["insert", "remove", "update", "move", "clear"]);
    expect(() => decodePatchOps(idx, { kind: "named", of: "Todo" }, Uint8Array.of(1, 0, 0, 0, 9))).toThrow(WireError);
  });

  it("applies them in order, each index relative to the list the previous one left", () => {
    expect(titles(applyPatch(list, ops([["insert", 1, 9, "x"]])))).toBe("axbc");
    expect(titles(applyPatch(list, ops([["remove", 0], ["remove", 0]])))).toBe("c");
    expect(titles(applyPatch(list, ops([["update", 1, 2, "B"]])))).toBe("aBc");
    expect(titles(applyPatch(list, ops([["clear"]])))).toBe("");
    // Move means remove at `from`, then insert so that it ends at `to`.
    expect(titles(applyPatch(list, ops([["move", 0, 2]])))).toBe("bca");
    expect(titles(applyPatch(list, ops([["move", 2, 0]])))).toBe("cab");
    expect(titles(applyPatch(list, ops([["insert", 3, 4, "d"], ["move", 3, 0]])))).toBe("dabc");
  });

  it("does not touch the list it was given", () => {
    const copy = JSON.stringify(list);
    applyPatch(list, ops([["remove", 0], ["clear"]]));
    expect(JSON.stringify(list)).toBe(copy);
  });

  it("says when an operation does not fit the list", () => {
    expect(() => applyPatch(list, ops([["insert", 4, 9, "x"]]))).toThrow(PatchMismatch);
    expect(() => applyPatch(list, ops([["remove", 3]]))).toThrow(PatchMismatch);
    expect(() => applyPatch(list, ops([["update", 3, 1, "x"]]))).toThrow(PatchMismatch);
    expect(() => applyPatch(list, ops([["move", 0, 3]]))).toThrow(PatchMismatch);
  });
});
