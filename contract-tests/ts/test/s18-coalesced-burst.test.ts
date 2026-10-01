import { expect, test } from "vitest";
import { ChangeOp, type DrainStats, codecs } from "@undra/runtime";
import { Stress, type StressMode, StressModeCodec, UndraIds } from "@playground/core";
import { boot } from "../src/harness.js";
import { RawStore, args, valueOf } from "../src/raw-store.js";
import { counters } from "../src/stats.js";
import { step } from "../src/wait.js";

// S18 coalesced burst: the core commits one change-set per transaction, and the mirror merges
// what arrived before it drains (ADR-031): a burst of 1,000 transactions on one signal is applied
// as one entry, with the exact final value, before the call that made it returns; a signal
// declared `no_coalesce` is applied entry by entry.

const Ids = UndraIds.Objects.Stress;

const burstArgs = (mode: StressMode, transactions: number) =>
  args((w) => {
    StressModeCodec.encode(w, mode);
    w.writeU32(transactions);
  });

test("S18 coalesced burst", async () => {
  const { core } = await boot();

  await step("1. raw: burst(Firehose, 1000) is applied as one entry, and transactions grew by 1000", async () => {
    const raw = await RawStore.open(core, Ids);
    raw.take(); // the initial change-set
    const before = await counters(core);
    const mirror = core.mirror.stats();
    await raw.call(Ids.burst, burstArgs("firehose", 1000));
    // Before anything else runs: the reply drained the mirror, and the raw callback ran once.
    expect(raw.received, "the raw mirror callback ran once").toBe(1);
    const entries = raw.take();
    expect(entries).toHaveLength(1);
    expect(entries[0]?.signalId).toBe(0);
    expect(entries[0]?.op).toBe(ChangeOp.FullValue);
    expect(valueOf(entries, 0, codecs.u64)).toBe(1000n);
    expect((await counters(core)).transactions - before.transactions, "one change-set per transaction").toBe(1000);
    const after = core.mirror.stats();
    expect(after.changeSetsReceived - mirror.changeSetsReceived).toBe(1000);
    expect(after.entriesApplied - mirror.entriesApplied).toBe(1);
    raw.close();
  });

  await step("2. generated: the final value is there when the call returns", async () => {
    const stress = await Stress.create(core);
    const drains: DrainStats[] = [];
    const stop = core.mirror.addDrainListener((s) => drains.push(s));
    await stress.burst("firehose", 1000);
    expect(stress.value.peek()).toBe(1000n);
    expect(drains.reduce((n, d) => n + d.changeSets, 0)).toBe(1000);
    expect(drains.reduce((n, d) => n + d.appliedEntries, 0)).toBe(1);
    stop();
    stress.close();
  });

  await step("3. generated: a no_coalesce signal is applied entry by entry", async () => {
    const stress = await Stress.create(core);
    const heard: number[] = [];
    stress.progress.subscribe((v) => heard.push(v));
    const applied = core.mirror.stats().entriesApplied;
    await stress.burst("progress", 10);
    expect(stress.progress.peek()).toBe(10);
    expect(core.mirror.stats().entriesApplied - applied, "every progress entry was applied").toBe(10);
    expect(heard, "and announced on its own").toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    stress.close();
  });
});
