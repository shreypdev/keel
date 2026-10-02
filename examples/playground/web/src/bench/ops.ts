/*
 * What the device benchmark measures, in one place. The ids, the sizes and the counts here are the
 * contract between the three runners (this one, `BenchRunner.swift` in the iOS app, `BenchRunner.kt`
 * in the Android app) and `scripts/bench-device-report.mjs`, which refuses a result file that lacks
 * an id. A runner may use other batch sizes (a browser's clock is far coarser than a phone's), and
 * says which in the file; it may not rename an operation.
 */

/** The ids of the operations every platform measures. */
export const OP_IDS = ["sync_call", "record_1kb", "keyed_insert_10k", "changeset_100"] as const;

/** An id of {@link OP_IDS}. */
export type OpId = (typeof OP_IDS)[number];

/** The rows that have a `[web."id"]` budget in bench/budgets.toml (R9): the operations, the runtime's own call, and the merged frame of the drain experiment. */
export const WEB_BUDGET_IDS = [...OP_IDS, "sync_call_runtime", "drain_frame"] as const;

/** Extra rows only the web measures: the runtime's own synchronous call, which generated TypeScript does not use. */
export const WEB_EXTRA_OP_IDS = ["sync_call_runtime"] as const;

/** The ADR-031 drain experiment: how many one-update keyed patches arrive per frame (100,000 a second at 60 Hz). */
export const DRAIN_UPDATES_PER_FRAME = 1_667;

/** Rows in the bench list at the start (`playground_core::bench::ROWS`). */
export const BENCH_ROWS = 10_000;

/** Where in the list the insert row lands: the middle. */
export const INSERT_AT = 5_000;

/** How many dirty signals the change-set row touches. */
export const TOUCHED_SIGNALS = 100;

/** The size of the payload the 1 KB row echoes. */
export const PAYLOAD_BYTES = 1_024;

/** How much of a run each part gets. `quick` is for the unit tests and a smoke run. */
export interface BenchConfig {
  /** Timed batches per operation (the percentiles are over this many). */
  readonly batches: number;
  /** Batches thrown away before the timed ones. */
  readonly warmupBatches: number;
  /** Operations per batch for each row; a browser's clock needs batches of milliseconds. */
  readonly batchSize: Readonly<Record<OpId | (typeof WEB_EXTRA_OP_IDS)[number], number>>;
  /** The list is put back to its 10,000 rows after this many inserts (outside the timed region). */
  readonly resetEveryInserts: number;
  /** Frames of the drain experiment that count, and the frames before them that do not. */
  readonly drainFrames: number;
  readonly drainWarmupFrames: number;
  /** After each frame of the drain experiment: calls that commit one update and calls that commit none, timed as two batches of this size. */
  readonly perEntryBatchSize: number;
  /** Reloads of the core inside one page, for the in-process cold start row. */
  readonly reloads: number;
}

/** The default run: about ten seconds of measuring. */
export const FULL: BenchConfig = {
  batches: 100,
  warmupBatches: 10,
  batchSize: { sync_call: 1_000, sync_call_runtime: 5_000, record_1kb: 500, keyed_insert_10k: 200, changeset_100: 100 },
  resetEveryInserts: 1_000,
  drainFrames: 240,
  drainWarmupFrames: 60,
  perEntryBatchSize: 20,
  reloads: 20,
};

/** A run that checks the plumbing in a fraction of a second. */
export const QUICK: BenchConfig = {
  batches: 4,
  warmupBatches: 1,
  batchSize: { sync_call: 20, sync_call_runtime: 20, record_1kb: 10, keyed_insert_10k: 10, changeset_100: 5 },
  resetEveryInserts: 20,
  drainFrames: 4,
  drainWarmupFrames: 1,
  perEntryBatchSize: 4,
  reloads: 2,
};
