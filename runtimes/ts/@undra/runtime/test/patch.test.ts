import { describe, expect, it } from "vitest";
import { codecs } from "../src/wire/codec.js";
import { PatchError, WireError } from "../src/wire/errors.js";
import { type PatchOp, applyPatch, decodePatch, encodePatch } from "../src/wire/index.js";
import { UndraReader } from "../src/wire/reader.js";
import { UndraWriter } from "../src/wire/writer.js";
import { type Todo, todoCodec } from "./fixtures.js";
import { embedded, expectWireError, fromHex, prng, randInt, toHex } from "./helpers.js";

const i32 = codecs.i32;

function encode<T>(ops: readonly PatchOp<T>[], item: Parameters<typeof encodePatch<T>>[2]): Uint8Array {
  const w = new UndraWriter();
  encodePatch(w, ops, item);
  return w.finish();
}

function decode<T>(bytes: Uint8Array, item: Parameters<typeof decodePatch<T>>[1]): PatchOp<T>[] {
  const r = new UndraReader(bytes);
  const ops = decodePatch(r, item);
  r.finish();
  return ops;
}

describe("keyed patch wire format", () => {
  const vectorOps: PatchOp<number>[] = [
    { op: "insert", index: 0, item: 5 },
    { op: "remove", index: 1 },
    { op: "move", from: 0, to: 1 },
    { op: "clear" },
  ];
  const vectorHex = "04000000" + "00" + "00000000" + "05000000" + "01" + "01000000" + "03" + "00000000" + "01000000" + "04";

  it("encodes the contract vector", () => {
    expect(toHex(encode(vectorOps, i32))).toBe(vectorHex);
  });

  it("decodes the contract vector", () => {
    expect(decode(fromHex(vectorHex), i32)).toEqual(vectorOps);
    expect(decode(embedded(fromHex(vectorHex)), i32)).toEqual(vectorOps);
  });

  it("uses op tags 0 insert, 1 remove, 2 update, 3 move, 4 clear", () => {
    const tag = (op: PatchOp<number>): string => toHex(encode([op], i32)).slice(8, 10);
    expect(tag({ op: "insert", index: 0, item: 1 })).toBe("00");
    expect(tag({ op: "remove", index: 0 })).toBe("01");
    expect(tag({ op: "update", index: 0, item: 1 })).toBe("02");
    expect(tag({ op: "move", from: 0, to: 1 })).toBe("03");
    expect(tag({ op: "clear" })).toBe("04");
  });

  it("round-trips an empty patch and every op with structured items", () => {
    expect(toHex(encode([], i32))).toBe("00000000");
    expect(decode(encode([], i32), i32)).toEqual([]);

    const todo = (n: number): Todo => ({
      id: `00000000-0000-0000-0000-${n.toString().padStart(12, "0")}`,
      title: `todo \u{1f30a} ${n}`,
      done: n % 2 === 0,
    });
    const ops: PatchOp<Todo>[] = [
      { op: "insert", index: 0, item: todo(1) },
      { op: "update", index: 0, item: todo(2) },
      { op: "insert", index: 1, item: todo(3) },
      { op: "move", from: 1, to: 0 },
      { op: "remove", index: 1 },
      { op: "clear" },
      { op: "insert", index: 0, item: todo(4) },
    ];
    expect(decode(encode(ops, todoCodec), todoCodec)).toEqual(ops);
  });

  it("round-trips index extremes", () => {
    const ops: PatchOp<number>[] = [
      { op: "remove", index: 0xffffffff },
      { op: "move", from: 0xffffffff, to: 0 },
      { op: "insert", index: 0xffffffff, item: -1 },
    ];
    expect(decode(encode(ops, i32), i32)).toEqual(ops);
  });

  it("decodePatch leaves trailing bytes to the caller", () => {
    const r = new UndraReader(fromHex(`${vectorHex}aabb`));
    decodePatch(r, i32);
    expect(r.remaining).toBe(2);
    expectWireError(() => r.finish(), "trailing_bytes", { count: 2 });
  });

  it("decodes a patch embedded after other data on the same reader", () => {
    const w = new UndraWriter();
    w.writeU32(99);
    encodePatch(w, vectorOps, i32);
    w.writeU8(7);
    const r = new UndraReader(w.finish());
    expect(r.readU32()).toBe(99);
    expect(decodePatch(r, i32)).toEqual(vectorOps);
    expect(r.readU8()).toBe(7);
    r.finish();
  });
});

describe("keyed patch malformed input", () => {
  it("rejects an unknown op tag with its offset", () => {
    for (const tag of [5, 6, 255]) {
      expectWireError(() => decode(fromHex(`01000000${tag.toString(16).padStart(2, "0")}`), i32), "invalid_tag", {
        tag,
        at: 4,
        ty: "PatchOp",
      });
    }
  });

  it("rejects a count larger than the remaining input", () => {
    expectWireError(() => decode(fromHex("02000000 04"), i32), "length_too_large", { len: 2 });
    expectWireError(() => decode(fromHex("ffffffff"), i32), "length_too_large");
  });

  it("rejects every strict prefix of a valid patch", () => {
    const bytes = encode(
      [
        { op: "insert", index: 3, item: 9 },
        { op: "update", index: 1, item: 8 },
        { op: "remove", index: 2 },
        { op: "move", from: 4, to: 5 },
        { op: "clear" },
      ],
      i32,
    );
    for (let n = 0; n < bytes.length; n++) {
      expect(() => decode(bytes.subarray(0, n), i32), `prefix ${n}`).toThrow(WireError);
    }
  });

  it("propagates an item decoding failure", () => {
    expectWireError(() => decode(fromHex("01000000 00 00000000 02"), codecs.bool), "invalid_tag", {
      tag: 2,
      ty: "bool",
    });
  });

  it("finish reports trailing bytes when the caller checks", () => {
    expectWireError(() => decode(fromHex("00000000 ff"), i32), "trailing_bytes", { count: 1 });
  });
});

describe("applyPatch", () => {
  const list = ["a", "b", "c"];

  it("inserts at the front, middle and end", () => {
    expect(applyPatch(list, [{ op: "insert", index: 0, item: "x" }])).toEqual(["x", "a", "b", "c"]);
    expect(applyPatch(list, [{ op: "insert", index: 2, item: "x" }])).toEqual(["a", "b", "x", "c"]);
    expect(applyPatch(list, [{ op: "insert", index: 3, item: "x" }])).toEqual(["a", "b", "c", "x"]);
    expect(applyPatch([], [{ op: "insert", index: 0, item: "x" }])).toEqual(["x"]);
  });

  it("removes and updates", () => {
    expect(applyPatch(list, [{ op: "remove", index: 0 }])).toEqual(["b", "c"]);
    expect(applyPatch(list, [{ op: "remove", index: 2 }])).toEqual(["a", "b"]);
    expect(applyPatch(list, [{ op: "update", index: 1, item: "B" }])).toEqual(["a", "B", "c"]);
    expect(applyPatch(["z"], [{ op: "remove", index: 0 }])).toEqual([]);
  });

  it("moves an item so that it ends up at index `to`", () => {
    const five = ["a", "b", "c", "d", "e"];
    expect(applyPatch(five, [{ op: "move", from: 0, to: 4 }])).toEqual(["b", "c", "d", "e", "a"]);
    expect(applyPatch(five, [{ op: "move", from: 4, to: 0 }])).toEqual(["e", "a", "b", "c", "d"]);
    expect(applyPatch(five, [{ op: "move", from: 1, to: 3 }])).toEqual(["a", "c", "d", "b", "e"]);
    expect(applyPatch(five, [{ op: "move", from: 3, to: 1 }])).toEqual(["a", "d", "b", "c", "e"]);
    expect(applyPatch(five, [{ op: "move", from: 2, to: 2 }])).toEqual(five);
    for (let from = 0; from < 5; from++) {
      for (let to = 0; to < 5; to++) {
        expect(applyPatch(five, [{ op: "move", from, to }])[to]).toBe(five[from]);
      }
    }
  });

  it("clears", () => {
    expect(applyPatch(list, [{ op: "clear" }])).toEqual([]);
    expect(applyPatch([], [{ op: "clear" }])).toEqual([]);
  });

  it("applies operations in sequence, each seeing the list left by the previous one", () => {
    const ops: PatchOp<string>[] = [
      { op: "insert", index: 0, item: "x" }, // x a b c
      { op: "remove", index: 1 }, // x b c
      { op: "move", from: 0, to: 2 }, // b c x
      { op: "update", index: 1, item: "C" }, // b C x
      { op: "insert", index: 3, item: "y" }, // b C x y
    ];
    expect(applyPatch(list, ops)).toEqual(["b", "C", "x", "y"]);
  });

  it("supports a clear followed by inserts (a full replacement)", () => {
    const ops: PatchOp<string>[] = [
      { op: "clear" },
      { op: "insert", index: 0, item: "p" },
      { op: "insert", index: 1, item: "q" },
    ];
    expect(applyPatch(list, ops)).toEqual(["p", "q"]);
  });

  it("applies the contract vector to a list", () => {
    const ops: PatchOp<number>[] = [
      { op: "insert", index: 0, item: 5 },
      { op: "remove", index: 1 },
      { op: "move", from: 0, to: 1 },
    ];
    // [1, 2] -> [5, 1, 2] -> [5, 2] -> [2, 5]
    expect(applyPatch([1, 2], ops)).toEqual([2, 5]);
  });

  it("never modifies its input and always returns a new array", () => {
    const input = ["a", "b", "c"];
    const snapshot = [...input];
    const out = applyPatch(input, [{ op: "clear" }, { op: "insert", index: 0, item: "z" }]);
    expect(input).toEqual(snapshot);
    expect(out).not.toBe(input);
    expect(applyPatch(input, [])).not.toBe(input);
    expect(applyPatch(input, [])).toEqual(input);
    const frozen = Object.freeze(["a", "b"]);
    expect(applyPatch(frozen, [{ op: "remove", index: 0 }])).toEqual(["b"]);
  });

  it("keeps the identity of items it does not touch and does not clone inserted ones", () => {
    const a = { id: 1 };
    const b = { id: 2 };
    const c = { id: 3 };
    const inserted = { id: 4 };
    const out = applyPatch([a, b, c], [{ op: "insert", index: 1, item: inserted }, { op: "remove", index: 3 }]);
    expect(out).toHaveLength(3);
    expect(out[0]).toBe(a);
    expect(out[1]).toBe(inserted);
    expect(out[2]).toBe(b);
  });

  it("rejects out-of-bounds indices with a PatchError describing the operation", () => {
    const cases: [PatchOp<string>, number][] = [
      [{ op: "insert", index: 4, item: "x" }, 4],
      [{ op: "remove", index: 3 }, 3],
      [{ op: "update", index: 3, item: "x" }, 3],
      [{ op: "move", from: 3, to: 0 }, 3],
      [{ op: "move", from: 0, to: 3 }, 3],
    ];
    for (const [op, index] of cases) {
      let error: unknown;
      try {
        applyPatch(list, [op]);
      } catch (e) {
        error = e;
      }
      expect(error, op.op).toBeInstanceOf(PatchError);
      const err = error as PatchError;
      expect(err.opIndex).toBe(0);
      expect(err.op).toBe(op.op);
      expect(err.index).toBe(index);
      expect(err.length).toBe(3);
      expect(err.message).toContain(op.op);
    }
  });

  it("reports the position of the failing operation and the list length at that point", () => {
    let error: unknown;
    try {
      applyPatch(list, [{ op: "clear" }, { op: "insert", index: 0, item: "x" }, { op: "remove", index: 1 }]);
    } catch (e) {
      error = e;
    }
    expect(error).toBeInstanceOf(PatchError);
    expect(error).toMatchObject({ opIndex: 2, op: "remove", index: 1, length: 1 });
  });

  it("rejects negative, fractional and non-numeric indices", () => {
    for (const index of [-1, 0.5, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(() => applyPatch(list, [{ op: "remove", index }]), String(index)).toThrow(PatchError);
      expect(() => applyPatch(list, [{ op: "insert", index, item: "x" }]), String(index)).toThrow(PatchError);
      expect(() => applyPatch(list, [{ op: "update", index, item: "x" }]), String(index)).toThrow(PatchError);
      expect(() => applyPatch(list, [{ op: "move", from: index, to: 0 }]), String(index)).toThrow(PatchError);
    }
  });

  it("does not partially apply: a failing patch leaves the input untouched", () => {
    const input = ["a", "b"];
    expect(() => applyPatch(input, [{ op: "remove", index: 0 }, { op: "remove", index: 5 }])).toThrow(PatchError);
    expect(input).toEqual(["a", "b"]);
  });

  it("remove or update on an empty list is out of bounds", () => {
    expect(() => applyPatch([], [{ op: "remove", index: 0 }])).toThrow(PatchError);
    expect(() => applyPatch([], [{ op: "update", index: 0, item: 1 }])).toThrow(PatchError);
    expect(() => applyPatch([], [{ op: "move", from: 0, to: 0 }])).toThrow(PatchError);
  });
});

describe("keyed patch end to end", () => {
  /** Reference model: applies each op by rebuilding the list, independent of splice. */
  function model(list: readonly number[], op: PatchOp<number>): number[] {
    switch (op.op) {
      case "insert":
        return [...list.slice(0, op.index), op.item, ...list.slice(op.index)];
      case "remove":
        return [...list.slice(0, op.index), ...list.slice(op.index + 1)];
      case "update":
        return list.map((v, i) => (i === op.index ? op.item : v));
      case "move": {
        const without = [...list.slice(0, op.from), ...list.slice(op.from + 1)];
        return [...without.slice(0, op.to), list[op.from] as number, ...without.slice(op.to)];
      }
      case "clear":
        return [];
    }
  }

  it("random valid patches survive encode, decode and apply and match the reference model", () => {
    const rand = prng(77);
    for (let round = 0; round < 400; round++) {
      const start = Array.from({ length: randInt(rand, 8) }, () => randInt(rand, 1000) - 500);
      let current = start;
      const ops: PatchOp<number>[] = [];
      const count = randInt(rand, 12);
      for (let i = 0; i < count; i++) {
        const len = current.length;
        const choices: PatchOp<number>[] = [
          { op: "insert", index: randInt(rand, len + 1), item: randInt(rand, 1000) - 500 },
          { op: "clear" },
        ];
        if (len > 0) {
          choices.push(
            { op: "remove", index: randInt(rand, len) },
            { op: "update", index: randInt(rand, len), item: randInt(rand, 1000) - 500 },
            { op: "move", from: randInt(rand, len), to: randInt(rand, len) },
          );
        }
        const op = choices[randInt(rand, choices.length)] as PatchOp<number>;
        ops.push(op);
        current = model(current, op);
      }
      const decoded = decode(encode(ops, i32), i32);
      expect(decoded).toEqual(ops);
      expect(applyPatch(start, decoded)).toEqual(current);
    }
  });
});
