import type { DrainStats } from "@undra/runtime";
import { describe, expect, it } from "vitest";
import { DRAIN_UPDATES_PER_FRAME, OP_IDS, QUICK } from "./ops";
import { type BenchEnv, type BenchLike, run } from "./runner";

/** A fake bench store and clock: each call costs a fixed number of microseconds, and the "mirror" is a plain array. */
function fake(options: { dropInserts?: boolean } = {}) {
  let t = 0; // milliseconds on the fake clock
  const listeners: ((stats: DrainStats) => void)[] = [];
  let rows = Array.from({ length: 10_000 }, (_, i) => ({ id: i + 1 }));
  let nextId = 10_001;
  let counter = 0;
  const cost = (microseconds: number) => {
    t += microseconds / 1000;
  };
  const bench: BenchLike = {
    rows: { peek: () => rows },
    s099: { peek: () => counter },
    benchAdd: (a, b) => (cost(2), Promise.resolve(a + b)),
    benchEchoBytes: (data) => (cost(5), Promise.resolve(data.slice())),
    benchTouchSignals: () => ((counter += 1), cost(40), Promise.resolve()),
    benchListInsert: (at) => {
      cost(10);
      if (options.dropInserts !== true) rows = [...rows.slice(0, at), { id: nextId++ }, ...rows.slice(at)];
      return Promise.resolve();
    },
    benchListReset: () => {
      rows = Array.from({ length: 10_000 }, (_, i) => ({ id: i + 1 }));
      nextId = 10_001;
      counter = 0;
      return Promise.resolve();
    },
    benchListUpdateBurst: (n) => {
      cost(n === 0 ? 1 : n * 1.5);
      if (n > 0) for (const l of listeners) l({ changeSets: n, entries: n, appliedEntries: 1, durationMs: 0.2 });
      return Promise.resolve();
    },
  };
  const env: BenchEnv = {
    bench,
    now: () => t,
    nextFrame: () => Promise.resolve(),
    syncAdd: (a, b) => (cost(0.1), a + b),
    onDrain: (listener) => {
      listeners.push(listener);
      return () => listeners.splice(listeners.indexOf(listener), 1);
    },
  };
  return env;
}

describe("the web device benchmark", () => {
  it("measures every row, by the fake clock", async () => {
    const result = await run(fake(), QUICK);
    expect(result.schema).toBe("undra-device-bench-raw/1");
    expect(result.ops.map((op) => op.id)).toEqual([...OP_IDS.slice(0, 1), "sync_call_runtime", ...OP_IDS.slice(1)]);
    const byId = Object.fromEntries(result.ops.map((op) => [op.id, op]));
    expect(byId["sync_call"]?.p50).toBeCloseTo(2000, 6); // 2 microseconds a call
    expect(byId["sync_call_runtime"]?.p50).toBeCloseTo(100, 6);
    expect(byId["record_1kb"]?.p50).toBeCloseTo(5000, 6);
    expect(byId["keyed_insert_10k"]?.p50).toBeCloseTo(10_000, 6);
    expect(byId["changeset_100"]?.p50).toBeCloseTo(40_000, 6);
    for (const op of result.ops) {
      expect(op.mode).toBe("batched");
      expect(op.samples).toBe(QUICK.batches);
    }
  });

  it("reports the drain experiment: one drain of 1,667 entries applied once, and the cost of an unmerged update", async () => {
    const { drain } = await run(fake(), QUICK);
    expect(drain.updates_per_frame).toBe(DRAIN_UPDATES_PER_FRAME);
    expect(drain.frames).toBe(QUICK.drainFrames);
    expect(drain.merged.change_sets_per_frame_p50).toBe(DRAIN_UPDATES_PER_FRAME);
    expect(drain.merged.entries_per_frame_p50).toBe(DRAIN_UPDATES_PER_FRAME);
    expect(drain.merged.applied_per_frame_p50).toBe(1);
    expect(drain.merged.drains_per_frame_p50).toBe(1);
    expect(drain.merged.frame_ns.p50).toBeCloseTo(DRAIN_UPDATES_PER_FRAME * 1.5 * 1000, 3);
    expect(drain.merged.drain_ns.p50).toBeCloseTo(200_000, 3);
    // One update on its own costs 1.5 us minus the 1 us of an empty call on the fake clock: 0.5 us.
    expect(drain.unmerged_estimate.per_entry_ns.p50).toBeCloseTo(500, 3);
    expect(drain.unmerged_estimate.frame_ns).toBeCloseTo(DRAIN_UPDATES_PER_FRAME * 500, 3);
    expect(drain.ratio_unmerged_over_merged).toBeCloseTo(500 / 1500, 6);
  });

  it("refuses to report a run in which the mirror did not change", async () => {
    await expect(run(fake({ dropInserts: true }), QUICK)).rejects.toThrow(/device bench check failed.*rows after/);
  });
});
