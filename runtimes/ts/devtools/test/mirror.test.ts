import { describe, expect, it } from "vitest";
import { WireError } from "@undra/runtime/wire";
import { Mirror } from "../src/mirror.js";
import { changeSet, full, i32Bytes, index, listBytes, patch } from "./helpers.js";

const COUNTER = 5n;
const TODOS = 6n;

function mirror(): Mirror {
  const m = new Mirror(index());
  m.setStores([
    { handle: COUNTER, typeId: 10 },
    { handle: TODOS, typeId: 11 },
  ]);
  return m;
}

describe("the page's copy of the stores", () => {
  it("applies a full value and says what it replaced", () => {
    const m = mirror();
    const first = m.apply(changeSet(1n, [full(COUNTER, 0, i32Bytes(5))]));
    expect(first.txn).toBe(1n);
    expect(first.changes).toMatchObject([{ store: "Counter", signal: "count", op: "full", before: undefined, after: 5 }]);
    const second = m.apply(changeSet(2n, [full(COUNTER, 0, i32Bytes(8))]));
    expect(second.changes[0]).toMatchObject({ before: 5, after: 8 });
    expect(m.stores.get(COUNTER)?.signals.get(0)?.value).toBe(8);
  });

  it("applies a transaction's entries together, in order", () => {
    const m = mirror();
    const { changes } = m.apply(changeSet(1n, [full(COUNTER, 0, i32Bytes(1)), full(COUNTER, 0, i32Bytes(2)), full(TODOS, 0, listBytes([[1, "a"]]))]));
    expect(changes.map((c) => c.after)).toEqual([1, 2, [{ id: 1, title: "a", done: false }]]);
  });

  it("applies a keyed patch to the list it holds", () => {
    const m = mirror();
    m.apply(changeSet(1n, [full(TODOS, 0, listBytes([[1, "a"], [2, "b"]]))]));
    const { changes } = m.apply(changeSet(2n, [patch(TODOS, 0, [["insert", 2, 3, "c"], ["remove", 0]])]));
    expect(changes[0]).toMatchObject({ op: "patch", patch: [{ op: "insert" }, { op: "remove" }] });
    expect((changes[0]?.after as { id: number }[]).map((t) => t.id)).toEqual([2, 3]);
  });

  it("keeps a list as it was when a patch does not fit, and says it is out of step", () => {
    const m = mirror();
    m.apply(changeSet(1n, [full(TODOS, 0, listBytes([[1, "a"]]))]));
    const { changes } = m.apply(changeSet(2n, [patch(TODOS, 0, [["remove", 5]])]));
    expect(changes[0]?.error).toMatch(/out of step/);
    expect((m.stores.get(TODOS)?.signals.get(0)?.value as unknown[]).length).toBe(1);
  });

  it("holds entries for a store the server has not announced and applies them when it does", () => {
    const m = new Mirror(index());
    expect(m.apply(changeSet(1n, [full(COUNTER, 0, i32Bytes(7))])).changes).toEqual([]);
    const replayed = m.setStores([{ handle: COUNTER, typeId: 10 }]);
    expect(replayed).toMatchObject([{ signal: "count", after: 7 }]);
  });

  it("forgets a store the core dropped", () => {
    const m = mirror();
    m.setStores([{ handle: TODOS, typeId: 11 }]);
    expect([...m.stores.keys()]).toEqual([TODOS]);
  });

  it("shows a store of a type the schema does not know as raw bytes instead of failing", () => {
    const m = new Mirror(index());
    m.setStores([{ handle: 9n, typeId: 999 }]);
    const { changes } = m.apply(changeSet(1n, [full(9n, 3, Uint8Array.of(1, 2))]));
    expect(changes[0]).toMatchObject({ store: "store 999", signal: "signal 3", after: Uint8Array.of(1, 2) });
  });

  it("refuses a malformed change-set whole", () => {
    const m = mirror();
    expect(() => m.apply(Uint8Array.of(1, 2, 3))).toThrow(WireError);
    // A value that does not decode is reported on its change; the others in the transaction still apply.
    const { changes } = m.apply(changeSet(1n, [full(COUNTER, 0, Uint8Array.of(1)), full(COUNTER, 0, i32Bytes(4))]));
    expect(changes[0]?.error).toBeDefined();
    expect(changes[1]?.after).toBe(4);
  });
});
