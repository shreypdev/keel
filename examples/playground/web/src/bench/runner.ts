import { type DrainStats } from "@undra/runtime";
import { BENCH_ROWS, type BenchConfig, DRAIN_UPDATES_PER_FRAME, INSERT_AT, PAYLOAD_BYTES, TOUCHED_SIGNALS } from "./ops";
import { type OpResult, type Summary, opResult, summarize } from "./stats";

/*
 * The web half of the device benchmark: the blueprint's section 14 operations, measured the way the
 * app experiences them, through the generated `Bench` store and the runtime's mirror, in this page's
 * own thread (`wasm-main`). Nothing here is Chromium-specific; the Playwright harness
 * (`bench/bench.spec.ts`) is only what loads the page and collects the object `run` returns.
 *
 * HOW A ROW IS TIMED. `performance.now()` is rounded by the browser (100 microseconds in Chrome unless the
 * page is cross-origin isolated, 5 when it is), so a row is timed as a batch of calls divided by the
 * batch's size, and the percentiles are over batches: a batch of 1,000 calls of 2 microseconds is 2 ms,
 * twenty clock steps at the coarse setting. The file says the batch size of every row and the clock's
 * step. The generated TypeScript calls (`await bench.benchAdd(..)`) are asynchronous by construction;
 * `sync_call_runtime` is the runtime's `callSync`, the blueprint's "in-thread" row, which generated code
 * does not use.
 *
 * EVERY ROW IS CHECKED. After each batch the mirror's copy of the state is compared with what the batch
 * did (the sum of the replies, the length of the list, the counter), so a number is never reported for a
 * run in which the apply did not happen.
 */

/** The part of the generated `Bench` store the benchmark uses (the generated class satisfies it). */
export interface BenchLike {
  /** The 10,000-row list, as the mirror holds it. */
  readonly rows: { peek(): readonly { readonly id: number }[] };
  /** The hundredth counter: the last of the 100 the change-set row touches. */
  readonly s099: { peek(): number };
  benchAdd(a: number, b: number): Promise<number>;
  benchEchoBytes(data: Uint8Array): Promise<Uint8Array>;
  benchTouchSignals(k: number): Promise<void>;
  benchListInsert(i: number): Promise<void>;
  benchListUpdateBurst(n: number): Promise<void>;
  benchListReset(): Promise<void>;
}

/** What the runner needs from the page. */
export interface BenchEnv {
  /** The bench store, observed. */
  readonly bench: BenchLike;
  /** `performance.now()`, in milliseconds. */
  readonly now: () => number;
  /** Resolves at the start of the next display frame (`requestAnimationFrame`). */
  readonly nextFrame: () => Promise<void>;
  /** The runtime's synchronous call of `Bench.benchAdd` (no promise), or `undefined` when the mode has none. */
  readonly syncAdd: ((a: number, b: number) => number) | undefined;
  /** `core.mirror.addDrainListener`: returns the function that removes the listener. */
  readonly onDrain: (listener: (stats: DrainStats) => void) => () => void;
}

/** What `drain` reports; see `.10x/adrs/ADR-031-frame-coalesced-delivery.md`. */
export interface DrainResult {
  readonly rows: number;
  readonly updates_per_frame: number;
  readonly frames: number;
  readonly warmup_frames: number;
  /** Who produces the patches and where the time is counted. */
  readonly producer: string;
  readonly merged: {
    /** Per frame, nanoseconds: the main thread's whole cost of the frame's burst. */
    readonly frame_ns: Summary;
    /** Per drain, nanoseconds, as the mirror's drain listener timed it (as coarse as the clock). */
    readonly drain_ns: Summary;
    readonly change_sets_per_frame_p50: number;
    readonly entries_per_frame_p50: number;
    readonly applied_per_frame_p50: number;
    readonly drains_per_frame_p50: number;
  };
  readonly unmerged_estimate: {
    /** The cost of one update that is not merged with any other, nanoseconds. */
    readonly per_entry_ns: Summary;
    /** `updates_per_frame` times the median per-entry cost. */
    readonly frame_ns: number;
    /** `updates_per_frame` times the mean per-entry cost. */
    readonly frame_ns_mean: number;
    readonly method: string;
  };
  /** Estimated unmerged frame (by the median entry) over measured merged frame (median). */
  readonly ratio_unmerged_over_merged: number;
  /** The same by the mean entry. */
  readonly ratio_unmerged_over_merged_mean: number;
  readonly note: string;
}

/** What `run` returns: the runner's half of the result file. */
export interface RawResult {
  readonly schema: "undra-device-bench-raw/1";
  readonly platform: "web";
  readonly runtime: "wasm-main";
  readonly timer: { readonly kind: string; readonly resolution_ns: number; readonly overhead_ns: number };
  readonly config: BenchConfig;
  readonly ops: readonly OpResult[];
  readonly drain: DrainResult;
}

const median = (values: readonly number[]): number => summarize(values).p50;

/** Times `batches` batches of `size` operations; returns the per-operation nanoseconds of each. `before` and `after` run outside the clock. */
async function timeBatches(
  env: BenchEnv,
  config: BenchConfig,
  size: number,
  run: (size: number) => Promise<void>,
  hooks: { readonly before?: () => Promise<void>; readonly after?: (size: number) => void } = {},
): Promise<number[]> {
  const perOp: number[] = [];
  for (let b = 0; b < config.warmupBatches + config.batches; b++) {
    await hooks.before?.();
    const t0 = env.now();
    await run(size);
    const t1 = env.now();
    hooks.after?.(size);
    if (b >= config.warmupBatches) perOp.push(((t1 - t0) * 1e6) / size);
  }
  return perOp;
}

function fail(message: string): never {
  throw new Error(`device bench check failed: ${message}`);
}

/** The smallest step of `now()` in nanoseconds, and what a pair of reads costs. */
export function timerFacts(now: () => number): { resolution_ns: number; overhead_ns: number } {
  let smallest = Number.POSITIVE_INFINITY;
  let last = now();
  for (let reads = 0, seen = 0; seen < 5 && reads < 500_000; reads++) {
    const t = now();
    if (t > last) {
      smallest = Math.min(smallest, t - last);
      last = t;
      seen++;
    }
  }
  // A pair of reads, averaged over a few thousand: what timing one operation on its own would add.
  const reads = 20_000;
  const t0 = now();
  for (let i = 0; i < reads; i++) now();
  const t1 = now();
  return { resolution_ns: Number.isFinite(smallest) ? smallest * 1e6 : 0, overhead_ns: ((t1 - t0) * 1e6) / reads };
}

/** Measures the four blueprint rows (and the runtime's synchronous call) on `env.bench`. */
export async function measureOps(env: BenchEnv, config: BenchConfig): Promise<OpResult[]> {
  const { bench } = env;
  const out: OpResult[] = [];

  // ---- the handle call: add two numbers through the generated binding --------------------------
  {
    const size = config.batchSize.sync_call;
    let sum = 0;
    let expected = 0;
    const perOp = await timeBatches(
      env,
      config,
      size,
      async (n) => {
        for (let i = 0; i < n; i++) sum += await bench.benchAdd(i & 0xffff, 3);
      },
      {
        after: (n) => {
          for (let i = 0; i < n; i++) expected += (i & 0xffff) + 3;
          if (sum !== expected) fail(`benchAdd answered ${sum}, not ${expected}`);
        },
      },
    );
    out.push(opResult("sync_call", "batched", size, perOp, "`await bench.benchAdd(a, b)`: encode, the call, the reply, decode, through the generated asynchronous binding"));
  }
  if (env.syncAdd !== undefined) {
    const syncAdd = env.syncAdd;
    const size = config.batchSize.sync_call_runtime;
    let sum = 0;
    let expected = 0;
    const perOp = await timeBatches(
      env,
      config,
      size,
      () => {
        for (let i = 0; i < size; i++) sum += syncAdd(i & 0xffff, 3);
        return Promise.resolve();
      },
      {
        after: (n) => {
          for (let i = 0; i < n; i++) expected += (i & 0xffff) + 3;
          if (sum !== expected) fail(`callSync answered ${sum}, not ${expected}`);
        },
      },
    );
    out.push(opResult("sync_call_runtime", "batched", size, perOp, "`UndraCore.callSync`: the runtime's synchronous entry (no promise), which the generated TypeScript does not use; the blueprint's in-thread row"));
  }

  // ---- 1 KB round trip -------------------------------------------------------------------------
  {
    const size = config.batchSize.record_1kb;
    const payload = new Uint8Array(PAYLOAD_BYTES);
    for (let i = 0; i < payload.length; i++) payload[i] = (i * 31 + 7) & 0xff;
    const perOp = await timeBatches(env, config, size, async (n) => {
      for (let i = 0; i < n; i++) {
        const back = await bench.benchEchoBytes(payload);
        if (back.length !== PAYLOAD_BYTES || back[PAYLOAD_BYTES - 1] !== payload[PAYLOAD_BYTES - 1]) fail("benchEchoBytes returned another payload");
      }
    });
    out.push(opResult("record_1kb", "batched", size, perOp, "`await bench.benchEchoBytes(1,024 bytes)`: the payload crosses the boundary twice (a byte string, which is cheaper than a record of the same size)"));
  }

  // ---- keyed insert into the observed 10,000-row list, applied to the mirror ---------------------
  {
    const size = config.batchSize.keyed_insert_10k;
    await bench.benchListReset();
    let inserted = 0;
    const perOp = await timeBatches(
      env,
      config,
      size,
      async (n) => {
        for (let i = 0; i < n; i++) await bench.benchListInsert(INSERT_AT);
      },
      {
        before: async () => {
          if (inserted >= config.resetEveryInserts) {
            await bench.benchListReset();
            inserted = 0;
          }
        },
        after: (n) => {
          inserted += n;
          const rows = bench.rows.peek();
          if (rows.length !== BENCH_ROWS + inserted) fail(`the mirror holds ${rows.length} rows after ${inserted} inserts`);
          if ((rows[INSERT_AT]?.id ?? 0) <= BENCH_ROWS) fail("the newest insert is not at the insert position in the mirror");
        },
      },
    );
    out.push(opResult("keyed_insert_10k", "batched", size, perOp, `\`await bench.benchListInsert(${INSERT_AT})\` on about 10,000 rows: the call, the core's recorded insert, the one-operation patch, applied to the mirror's list before the call returns`));
  }

  // ---- a 100-signal change-set -----------------------------------------------------------------
  {
    const size = config.batchSize.changeset_100;
    await bench.benchListReset();
    let touched = 0;
    const perOp = await timeBatches(
      env,
      config,
      size,
      async (n) => {
        for (let i = 0; i < n; i++) await bench.benchTouchSignals(TOUCHED_SIGNALS);
      },
      {
        after: (n) => {
          touched += n;
          if (bench.s099.peek() !== touched) fail(`the mirror's hundredth counter is ${bench.s099.peek()} after ${touched} touches`);
        },
      },
    );
    out.push(opResult("changeset_100", "batched", size, perOp, "`await bench.benchTouchSignals(100)`: one call, one change-set of 100 entries, applied to 100 signals of the mirror before the call returns"));
  }
  return out;
}

/** The ADR-031 drain experiment, on the web: 1,667 one-update patches per frame. */
export async function measureDrain(env: BenchEnv, config: BenchConfig): Promise<DrainResult> {
  const { bench } = env;
  await bench.benchListReset();
  const frames: { wall: number; drains: DrainStats[] }[] = [];
  const perEntry: number[] = [];
  let current: DrainStats[] = [];
  const stop = env.onDrain((stats) => current.push(stats));
  try {
    for (let f = 0; f < config.drainWarmupFrames + config.drainFrames; f++) {
      await env.nextFrame();
      current = [];
      const t0 = env.now();
      await bench.benchListUpdateBurst(DRAIN_UPDATES_PER_FRAME);
      const t1 = env.now();
      const drains = current;
      current = []; // the single-entry drains below are not part of the frame
      // The cost of one update on its own, in the same minutes as the frames: calls that commit one update (one transaction, one drain of one
      // entry, the list copy) minus calls that commit none (the same call with nothing to drain), as two batches.
      const size = config.perEntryBatchSize;
      const a0 = env.now();
      for (let i = 0; i < size; i++) await bench.benchListUpdateBurst(1);
      const a1 = env.now();
      for (let i = 0; i < size; i++) await bench.benchListUpdateBurst(0);
      const a2 = env.now();
      if (f >= config.drainWarmupFrames) {
        frames.push({ wall: (t1 - t0) * 1e6, drains });
        perEntry.push(Math.max(0, ((a1 - a0) - (a2 - a1)) * 1e6) / size);
      }
    }
  } finally {
    stop();
  }
  const rows = bench.rows.peek();
  if (rows.length !== BENCH_ROWS) fail(`the update bursts changed the list's length to ${rows.length}`);

  const sum = (f: (d: DrainStats) => number) => frames.map((frame) => frame.drains.reduce((s, d) => s + f(d), 0));
  const changeSets = sum((d) => d.changeSets);
  if (changeSets.some((n) => n !== DRAIN_UPDATES_PER_FRAME)) fail(`a frame's drains consumed ${changeSets.join(",")} change-sets, not ${DRAIN_UPDATES_PER_FRAME}`);

  const perEntrySummary = summarize(perEntry);
  const mergedFrame = summarize(frames.map((frame) => frame.wall));
  const unmergedFrame = DRAIN_UPDATES_PER_FRAME * perEntrySummary.p50;
  const unmergedFrameMean = DRAIN_UPDATES_PER_FRAME * perEntrySummary.mean;
  return {
    rows: BENCH_ROWS,
    updates_per_frame: DRAIN_UPDATES_PER_FRAME,
    frames: config.drainFrames,
    warmup_frames: config.drainWarmupFrames,
    producer: "the page's own thread: one awaited call per frame commits the burst (wasm-main), so the whole burst is main-thread time",
    merged: {
      frame_ns: mergedFrame,
      drain_ns: summarize(frames.flatMap((frame) => frame.drains.map((d) => d.durationMs * 1e6))),
      change_sets_per_frame_p50: median(changeSets),
      entries_per_frame_p50: median(sum((d) => d.entries)),
      applied_per_frame_p50: median(sum((d) => d.appliedEntries)),
      drains_per_frame_p50: median(frames.map((frame) => frame.drains.length)),
    },
    unmerged_estimate: {
      per_entry_ns: perEntrySummary,
      frame_ns: unmergedFrame,
      frame_ns_mean: unmergedFrameMean,
      method:
        `a call that commits one update (one transaction, one drain of one entry, the list copy) minus a call that commits none, as two batches of ${config.perEntryBatchSize} after each frame of the experiment so that both are measured in the same minutes (${perEntrySummary.n} batches); ` +
        `times ${DRAIN_UPDATES_PER_FRAME}, by the median batch and by the mean batch. The runtime was not reverted: this is what applying every entry on its own would cost, from the entry cost measured here`,
    },
    ratio_unmerged_over_merged: unmergedFrame / mergedFrame.p50,
    ratio_unmerged_over_merged_mean: unmergedFrameMean / mergedFrame.p50,
    note: "merged frame = the awaited burst call of 1,667 transactions plus the drain it ends with, as the page experiences it",
  };
}

/** Runs everything `config` asks for; the page calls this once per load. */
export async function run(env: BenchEnv, config: BenchConfig): Promise<RawResult> {
  const timer = timerFacts(env.now);
  const ops = await measureOps(env, config);
  const drain = await measureDrain(env, config);
  return {
    schema: "undra-device-bench-raw/1",
    platform: "web",
    runtime: "wasm-main",
    timer: { kind: "performance.now", ...timer },
    config,
    ops,
    drain,
  };
}
