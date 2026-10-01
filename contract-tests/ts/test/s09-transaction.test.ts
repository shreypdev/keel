import { expect, test } from "vitest";
import { ChangeOp, codecs } from "@undra/runtime";
import { UndraIds, ParityCodec } from "@playground/core";
import { boot } from "../src/harness.js";
import { RawStore, type SignalUpdate, valueOf } from "../src/raw-store.js";
import { counters } from "../src/stats.js";
import { step } from "../src/wait.js";

// S09 transaction: a store command that writes several signals is one transaction and so one
// change-set, however many entries it carries; nothing is delivered while nobody observes.

const Counter = UndraIds.Objects.Counter;
const Bench = UndraIds.Objects.Bench;

/** The value a counter signal carries in a set of entries: `count` (0), `changes` (1), `parity` (2). */
function counterState(entries: readonly SignalUpdate[]): { count: number; changes: number; parity: string } {
  return { count: valueOf(entries, 0, codecs.i32), changes: valueOf(entries, 1, codecs.u32), parity: valueOf(entries, 2, ParityCodec) };
}

test("S09 transaction: a single change-set", async () => {
  const { core } = await boot();
  const counter = await RawStore.open(core, Counter);
  counter.take();

  await step("1. counter.add(5): three entries, one transaction", async () => {
    const before = await counters(core);
    const changeSets = core.mirror.changeSets;
    await counter.callWith(Counter.add, codecs.i32, 5);
    const entries = counter.take();
    expect(entries.map((e) => e.signalId)).toEqual([0, 1, 2]);
    expect(entries.every((e) => e.op === ChangeOp.FullValue)).toBe(true);
    expect(counterState(entries)).toEqual({ count: 5, changes: 1, parity: "odd" });
    expect((await counters(core)).transactions - before.transactions).toBe(1);
    expect(core.mirror.changeSets - changeSets, "and the runtime received one change-set").toBe(1);
  });

  await step("2. three separate calls are three transactions", async () => {
    const before = await counters(core);
    await counter.call(Counter.increment);
    await counter.call(Counter.increment);
    await counter.call(Counter.decrement);
    expect((await counters(core)).transactions - before.transactions).toBe(3);
    expect(counter.take()).toHaveLength(9);
  });

  await step("3. reset (two writes in one txn) is one transaction", async () => {
    const before = await counters(core);
    await counter.call(Counter.reset);
    const entries = counter.take();
    expect((await counters(core)).transactions - before.transactions).toBe(1);
    expect(entries.map((e) => e.signalId)).toEqual([0, 1, 2]);
    expect(counterState(entries)).toEqual({ count: 0, changes: 0, parity: "even" });
  });

  await step("4. bench_touch_signals delivers one entry per touched signal, in one transaction", async () => {
    const bench = await RawStore.open(core, Bench);
    bench.take(); // the initial change-set: the 10,000 rows and all 128 counters
    const touch = (k: number) => bench.callWith(Bench.benchTouchSignals, codecs.u32, k);

    const before = await counters(core);
    await touch(100);
    let entries = bench.take();
    expect((await counters(core)).transactions - before.transactions).toBe(1);
    expect(entries).toHaveLength(100);
    expect(entries.map((e) => e.signalId)).toEqual(Array.from({ length: 100 }, (_, i) => i + 1));
    expect(entries.every((e) => e.op === ChangeOp.FullValue && valueOf([e], e.signalId, codecs.u32) === 1)).toBe(true);

    await touch(1);
    entries = bench.take();
    expect(entries).toHaveLength(1);
    expect(entries[0]?.signalId).toBe(1);

    await touch(1000);
    entries = bench.take();
    expect(entries, "only 128 counters exist").toHaveLength(128);
    expect(entries.map((e) => e.signalId)).toEqual(Array.from({ length: 128 }, (_, i) => i + 1));
    bench.close();
  });

  await step("5. writes are not delivered while nothing observes; observing sends the current value once", async () => {
    const quiet = await RawStore.open(core, Counter, { observe: false });
    const before = await counters(core);
    const changeSets = core.mirror.changeSets;
    await quiet.callWith(Counter.add, codecs.i32, 1);
    expect((await counters(core)).transactions - before.transactions).toBe(0);
    expect(quiet.received).toBe(0);
    await quiet.observe(true);
    const entries = quiet.take();
    expect(core.mirror.changeSets - changeSets).toBe(1);
    expect(counterState(entries)).toEqual({ count: 1, changes: 1, parity: "odd" });
    quiet.close();
  });

  counter.close();
});
