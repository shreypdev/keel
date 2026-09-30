import { expect, test } from "vitest";
import { BigList, Counter, type Todo, Todos } from "@playground/core";
import { boot } from "../src/harness.js";
import { counters } from "../src/stats.js";
import { step } from "../src/wait.js";

// S11 computed: derived values (`visible`, `remaining`, `parity`, `count`) are computed in the
// core and arrive in the same change-set as the write that caused them; reading any signal is a
// local read that never crosses the boundary.

const titles = (todos: readonly Todo[]): string[] => todos.map((t) => t.title);

test("S11 computed", async () => {
  const { core } = await boot();
  const todos = await Todos.create(core);
  const changeSets = () => core.mirror.changeSets;

  const a = await todos.add("a");
  const b = await todos.add("b");
  const c = await todos.add("c");

  await step("1. remaining and visible follow adds and toggles", async () => {
    // Ids are lower-case canonical UUIDs whose leading bytes count up, so text order is creation order.
    expect(a.id < b.id && b.id < c.id, `ids count up: ${a.id} ${b.id} ${c.id}`).toBe(true);
    await todos.toggle(b.id);
    expect(todos.remaining.peek()).toBe(2);
    expect(todos.visible.peek()).toEqual([
      { ...a, done: false },
      { ...b, done: true },
      { ...c, done: false },
    ]);
  });

  await step("2. set_filter(Done): one change-set carries the filter and the recomputed visible list", async () => {
    const sets = changeSets();
    const transactions = (await counters(core)).transactions;
    const remainingChanges: number[] = [];
    const stop = todos.remaining.subscribe((n) => remainingChanges.push(n));
    await todos.setFilter("done");
    expect(changeSets() - sets, "exactly one change-set").toBe(1);
    expect((await counters(core)).transactions - transactions).toBe(1);
    expect(todos.filter.peek()).toBe("done");
    expect(titles(todos.visible.peek())).toEqual(["b"]);
    expect(todos.remaining.peek(), "remaining does not change").toBe(2);
    expect(remainingChanges, "and is not announced again").toEqual([]);
    stop();
  });

  await step("3. Active, then All", async () => {
    await todos.setFilter("active");
    expect(titles(todos.visible.peek())).toEqual(["a", "c"]);
    await todos.setFilter("all");
    expect(titles(todos.visible.peek())).toEqual(["a", "b", "c"]);
  });

  await step("4. remove and clear_done", async () => {
    await todos.remove(a.id);
    expect(titles(todos.visible.peek())).toEqual(["b", "c"]);
    expect(todos.remaining.peek()).toBe(1);
    await todos.clearDone();
    expect(titles(todos.visible.peek())).toEqual(["c"]);
    expect(titles(todos.todos.peek())).toEqual(["c"]);
    expect(todos.remaining.peek()).toBe(1);
  });

  await step("5. Counter.parity always agrees with count", async () => {
    const counter = await Counter.create(core);
    const expected = (n: number) => (Math.abs(n) % 2 === 0 ? "even" : "odd");
    for (const [amount, parity] of [
      [3, "odd"],
      [1, "even"],
      [-1, "odd"],
      [0, "odd"],
    ] as const) {
      await counter.add(amount);
      expect(counter.parity.peek()).toBe(parity);
      expect(counter.parity.peek()).toBe(expected(counter.count.peek()));
    }
    expect(counter.count.peek()).toBe(3);
    counter.close();
  });

  await step("6. reads never cross the boundary", async () => {
    const counter = await Counter.create(core);
    const list = await BigList.create(core);
    const before = await counters(core);
    let sink = 0;
    for (let i = 0; i < 1_000; i++) {
      sink += todos.visible.peek().length + todos.remaining.peek();
      sink += counter.parity.peek().length + list.items.peek().length;
      sink += todos.visible.get().length + list.count.get();
    }
    const after = await counters(core);
    expect(after.calls - before.calls, "crossings.calls after 1,000 reads of each signal").toBe(0);
    expect(after.transactions - before.transactions).toBe(0);
    expect(sink).toBeGreaterThan(0);
    counter.close();
    list.close();
  });

  todos.close();
});
