import { expect, test } from "vitest";
import { ChangeOp, UndraReader, type PatchOp, applyPatch, codecs, decodePatch, decodeValue } from "@undra/runtime";
import { BigList as BigListStore, type Item, ItemCodec, UndraIds } from "@playground/core";
import { boot } from "../src/harness.js";
import { RawStore, type SignalUpdate, args, valueOf } from "../src/raw-store.js";
import { step } from "../src/wait.js";

// S10 keyed patch: every one-row operation on a 10,000-row keyed list crosses as a keyed patch of
// exactly one operation, a few bytes long, and applying it to the host's copy gives the list the
// core has.

const BigList = UndraIds.Objects.BigList;
const Bench = UndraIds.Objects.Bench;
const vecItem = codecs.vec(ItemCodec);

const item = (id: number, label: string, version = 0): Item => ({ id, label, version });

/** The patch an entry carries; fails if the entry is not a keyed patch. */
function patchOf(entry: SignalUpdate): PatchOp<Item>[] {
  expect(entry.op, "the entry is a keyed patch").toBe(ChangeOp.KeyedPatch);
  const r = new UndraReader(entry.value);
  const ops = decodePatch(r, ItemCodec);
  r.finish();
  return ops;
}

/** The single entry of `signalId` in a change-set (the scenario counts entries, so two would be a failure). */
function only(entries: readonly SignalUpdate[], signalId: number): SignalUpdate {
  const found = entries.filter((e) => e.signalId === signalId);
  expect(found, `entries for signal ${signalId}`).toHaveLength(1);
  return found[0] as SignalUpdate;
}

/** A list the test keeps by doing to it what the core does, independently of the wire. */
class Model {
  constructor(readonly rows: Item[]) {}
  insert(index: number, row: Item): void {
    this.rows.splice(index, 0, row);
  }
  update(index: number, label: string): void {
    const row = this.rows[index] as Item;
    this.rows[index] = { ...row, label, version: row.version + 1 };
  }
  move(from: number, to: number): void {
    this.rows.splice(to, 0, ...this.rows.splice(from, 1));
  }
  remove(index: number): void {
    this.rows.splice(index, 1);
  }
}

test("S10 keyed patch", async ({ task }) => {
  const { core } = await boot();
  const list = await RawStore.open(core, BigList);
  // The host's copy of the list, built from the initial change-set, and the model beside it.
  let mirror: Item[] = [];
  let model: Model = new Model([]);

  /** Applies the `items` patch of `entries` to the host's copy and compares it, in full, with the model. */
  const applyAndCompare = (ops: PatchOp<Item>[]): void => {
    mirror = applyPatch(mirror, ops);
    expect(mirror).toHaveLength(model.rows.length);
    expect(mirror).toEqual(model.rows);
  };

  await step("1. the initial entry for items is a full value of 10,000 items", () => {
    const entries = list.take();
    const items = only(entries, 0);
    expect(items.op).toBe(ChangeOp.FullValue);
    mirror = decodeValue(vecItem, items.value);
    expect(mirror).toHaveLength(10_000);
    model = new Model(mirror.map((row) => ({ ...row })));
    expect(valueOf(entries, 1, codecs.u32)).toBe(10_000);
  });

  await step("2. insert_at(5000, 'fresh') is one Insert, under 100 bytes", async () => {
    const id = decodeValue(
      codecs.u32,
      await list.call(
        BigList.insertAt,
        args((w) => {
          w.writeU32(5000);
          w.writeStr("fresh");
        }),
      ),
    );
    expect(id).toBe(10_001);
    const entries = list.take();
    const items = only(entries, 0);
    const ops = patchOf(items);
    expect(ops).toEqual([{ op: "insert", index: 5000, item: item(10_001, "fresh") }]);
    expect(items.value.length, "the entry's value is a few bytes, not a list").toBeLessThan(100);
    expect(valueOf(entries, 1, codecs.u32)).toBe(10_001);
    model.insert(5000, item(10_001, "fresh"));
    applyAndCompare(ops);
  });

  await step("3. update_at(42, 'renamed') is one Update", async () => {
    await list.call(
      BigList.updateAt,
      args((w) => {
        w.writeU32(42);
        w.writeStr("renamed");
      }),
    );
    const entries = list.take();
    const ops = patchOf(only(entries, 0));
    expect(ops).toEqual([{ op: "update", index: 42, item: item(43, "renamed", 1) }]);
    model.update(42, "renamed");
    applyAndCompare(ops);
  });

  await step("4. move_item(10, 9000) is one Move", async () => {
    await list.call(
      BigList.moveItem,
      args((w) => {
        w.writeU32(10);
        w.writeU32(9000);
      }),
    );
    const entries = list.take();
    const ops = patchOf(only(entries, 0));
    expect(ops).toEqual([{ op: "move", from: 10, to: 9000 }]);
    model.move(10, 9000);
    applyAndCompare(ops);
  });

  await step("5. remove_at(0) is one Remove", async () => {
    await list.callWith(BigList.removeAt, codecs.u32, 0);
    const entries = list.take();
    const ops = patchOf(only(entries, 0));
    expect(ops).toEqual([{ op: "remove", index: 0 }]);
    model.remove(0);
    applyAndCompare(ops);
    expect(mirror).toHaveLength(10_000);
  });

  await step("6. the host's copy equals the model after every step (checked above), and at the end", () => {
    expect(mirror).toEqual(model.rows);
    expect(mirror[0]).toEqual(item(2, "Item 2"));
  });

  await step("7. Bench.bench_list_insert(123) is a one-operation patch on a 10,000-row list", async () => {
    const bench = await RawStore.open(core, Bench);
    const initial = bench.take();
    const rows = only(initial, 0);
    expect(rows.op).toBe(ChangeOp.FullValue);
    const before = decodeValue(vecItem, rows.value);
    expect(before).toHaveLength(10_000);
    await bench.callWith(Bench.benchListInsert, codecs.u32, 123);
    const entries = bench.take();
    const changed = only(entries, 0);
    const ops = patchOf(changed);
    expect(ops).toHaveLength(1);
    expect(ops[0]).toMatchObject({ op: "insert", index: 123 });
    expect(changed.value.length).toBeLessThan(100);
    expect(applyPatch(before, ops)).toHaveLength(10_001);
    bench.close();
  });

  await step("8. reset() after removing the first item is one Insert of Item(1)", async () => {
    const fresh = await RawStore.open(core, BigList);
    const start = decodeValue(vecItem, only(fresh.take(), 0).value);
    await fresh.callWith(BigList.removeAt, codecs.u32, 0);
    const afterRemove = applyPatch(start, patchOf(only(fresh.take(), 0)));
    expect(afterRemove).toHaveLength(9_999);
    await fresh.call(BigList.reset);
    const entries = fresh.take();
    const ops = patchOf(only(entries, 0));
    expect(ops).toEqual([{ op: "insert", index: 0, item: item(1, "Item 1") }]);
    expect(applyPatch(afterRemove, ops)).toEqual(start);
    fresh.close();
  });

  // Not part of the scenario, printed for the record: what one `update_at` costs on the 10,000-row list, end to
  // end, with nobody observing it (the write alone) and through a generated store (the call, the core's
  // diff, the change-set, the patch applied to the 10,000-item copy, the signal announced).
  const rounds = 500;
  const label = (i: number) => args((w) => {
    w.writeU32(i % 100);
    w.writeStr(`Round ${i}`);
  });
  const quiet = await RawStore.open(core, BigList, { observe: false });
  let started = performance.now();
  for (let i = 0; i < rounds; i++) await quiet.call(BigList.updateAt, label(i));
  const unobservedUs = ((performance.now() - started) * 1000) / rounds;
  quiet.close();

  const store = await BigListStore.create(core);
  started = performance.now();
  for (let i = 0; i < rounds; i++) await store.updateAt(i % 100, `Round ${i}`);
  const observedUs = ((performance.now() - started) * 1000) / rounds;
  expect(store.items.peek()[0]).toMatchObject({ label: "Round 400" });
  (task.meta.notes ??= []).push(
    `update_at on a 10,000-row list, mean over ${rounds} calls: ${unobservedUs.toFixed(0)} us unobserved, ${observedUs.toFixed(0)} us through an observing generated store`,
  );
  store.close();

  list.close();
});
