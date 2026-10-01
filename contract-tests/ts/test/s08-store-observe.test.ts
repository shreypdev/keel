import { expect, test } from "vitest";
import { ChangeOp, codecs } from "@undra/runtime";
import { BigList, Counter, FilterCodec, UndraIds, TodoCodec, Todos } from "@playground/core";
import { boot } from "../src/harness.js";
import { RawStore, valueOf } from "../src/raw-store.js";
import { counters } from "../src/stats.js";
import { sleep, step, waitFor } from "../src/wait.js";

// S08 store observe: observing a store delivers exactly one initial change-set with every signal
// as a full value, before `observe` resolves; observing off silences the store, observing on
// again sends the current values; releasing a store frees its handle.

const vecTodo = codecs.vec(TodoCodec);

test("S08 store observe: initial change-set", async () => {
  const { core } = await boot();

  await step("1. raw: one change-set, four full values, before observe resolves", async () => {
    const store = await RawStore.open(core, UndraIds.Objects.Todos, { observe: false });
    expect(store.received).toBe(0);
    const changeSetsBefore = core.mirror.changeSets;
    await store.observe(true);
    // No flush, no extra await: what the core delivered is already in the mirror.
    expect(store.received).toBe(4);
    const entries = store.take();
    expect(core.mirror.changeSets - changeSetsBefore, "one change-set").toBe(1);
    expect(entries.map((e) => e.signalId)).toEqual([0, 1, 2, 3]);
    expect(entries.every((e) => e.op === ChangeOp.FullValue)).toBe(true);
    expect(valueOf(entries, 0, vecTodo)).toEqual([]);
    expect(valueOf(entries, 1, FilterCodec)).toBe("all");
    expect(valueOf(entries, 2, vecTodo)).toEqual([]);
    expect(valueOf(entries, 3, codecs.u32)).toBe(0);
    store.close();
  });

  await step("2. Todos.create() has its values at once", async () => {
    const todos = await Todos.create(core);
    // No await between create() returning and these reads.
    expect(todos.todos.peek()).toEqual([]);
    expect(todos.filter.peek()).toBe("all");
    expect(todos.visible.peek()).toEqual([]);
    expect(todos.remaining.peek()).toBe(0);
    todos.close();
  });

  await step("3. Counter and BigList start as documented", async () => {
    const counter = await Counter.create(core);
    expect(counter.count.peek()).toBe(0);
    expect(counter.changes.peek()).toBe(0);
    expect(counter.parity.peek()).toBe("even");

    const list = await BigList.create(core);
    const items = list.items.peek();
    expect(items).toHaveLength(10_000);
    expect(items[0]).toEqual({ id: 1, label: "Item 1", version: 0 });
    expect(items[9_999]).toEqual({ id: 10_000, label: "Item 10000", version: 0 });
    expect(list.count.peek()).toBe(10_000);
    counter.close();
    list.close();
  });

  await step("4. observing off silences the store; observing on sends the current values once", async () => {
    const store = await RawStore.open(core, UndraIds.Objects.Todos);
    store.take();
    await store.observe(false);
    const changeSetsOff = core.mirror.changeSets;
    await store.callWith(UndraIds.Objects.Todos.add, codecs.string, "x");
    await sleep(200);
    expect(store.received, "nothing reaches a store nobody observes").toBe(0);
    expect(core.mirror.changeSets - changeSetsOff).toBe(0);

    await store.observe(true);
    const entries = store.take();
    expect(core.mirror.changeSets - changeSetsOff, "one change-set on observing again").toBe(1);
    expect(entries.map((e) => e.signalId)).toEqual([0, 1, 2, 3]);
    const todos = valueOf(entries, 0, vecTodo);
    expect(todos).toHaveLength(1);
    expect(todos[0]).toMatchObject({ title: "x", done: false });
    expect(valueOf(entries, 2, vecTodo)).toEqual(todos);
    expect(valueOf(entries, 3, codecs.u32)).toBe(1);
    store.close();
  });

  await step("5. releasing a store drops live_handles by one", async () => {
    const todos = await Todos.create(core);
    const before = await counters(core);
    todos.close();
    const after = await waitFor("the handle to be released", async () => {
      const c = await counters(core);
      return c.liveHandles === before.liveHandles - 1 && c;
    });
    expect(after.liveHandles).toBe(before.liveHandles - 1);
    // Releasing twice is harmless.
    todos.close();
    expect((await counters(core)).liveHandles).toBe(after.liveHandles);
  });
});
