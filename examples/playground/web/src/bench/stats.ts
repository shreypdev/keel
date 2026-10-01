/*
 * The statistics of the device benchmark: one function that turns a list of samples into the numbers
 * the result file carries, and the shape of that file's per-operation row. No DOM, no runtime import,
 * so it is unit-tested in Node. The native runners (Swift, Kotlin) compute the same numbers the same
 * way; `scripts/bench-device-report.mjs` checks every file against the schema it describes.
 */

/** The summary of a list of samples, in the unit of the samples. */
export interface Summary {
  /** How many samples. */
  readonly n: number;
  /** The smallest sample. */
  readonly min: number;
  /** The median (nearest rank). */
  readonly p50: number;
  /** The 90th percentile (nearest rank). */
  readonly p90: number;
  /** The 99th percentile (nearest rank): at 100 samples, the second largest. */
  readonly p99: number;
  /** The largest sample. */
  readonly max: number;
  /** The arithmetic mean. */
  readonly mean: number;
}

/**
 * The nearest-rank percentile `p` (0..1) of an ascending-sorted list: the value at rank
 * `ceil(p * n)`. `NaN` for an empty list.
 *
 * ```ts
 * percentile([10, 20, 30, 40], 0.5); // 20
 * ```
 */
export function percentile(sorted: readonly number[], p: number): number {
  if (sorted.length === 0) return Number.NaN;
  // The epsilon keeps 0.07 * 100 = 7.000000000000001 from rounding up to rank 8.
  const rank = Math.min(sorted.length, Math.max(1, Math.ceil(p * sorted.length - 1e-9)));
  return sorted[rank - 1] as number;
}

/** Summarises `samples` (any order). An empty list gives `NaN` everywhere and `n: 0`. */
export function summarize(samples: readonly number[]): Summary {
  const sorted = [...samples].sort((a, b) => a - b);
  const n = sorted.length;
  const mean = n === 0 ? Number.NaN : sorted.reduce((sum, v) => sum + v, 0) / n;
  return {
    n,
    min: sorted[0] ?? Number.NaN,
    p50: percentile(sorted, 0.5),
    p90: percentile(sorted, 0.9),
    p99: percentile(sorted, 0.99),
    max: sorted[n - 1] ?? Number.NaN,
    mean,
  };
}

/** How an operation was timed: each call on its own, or a batch of calls divided by its size. */
export type TimingMode = "each" | "batched";

/** One measured operation, as the result file records it (times are nanoseconds per operation). */
export interface OpResult {
  /** Stable id of the operation (`sync_call`, `record_1kb`, ...). */
  readonly id: string;
  /** How it was timed. */
  readonly mode: TimingMode;
  /** Operations per timed batch (1 when `mode` is `each`). */
  readonly batch: number;
  /** Timed batches, which is how many samples the percentiles are over. */
  readonly samples: number;
  /** Nanoseconds per operation: the smallest batch mean. */
  readonly min: number;
  /** Nanoseconds per operation: the median batch mean. */
  readonly p50: number;
  /** Nanoseconds per operation: the 90th percentile of batch means. */
  readonly p90: number;
  /** Nanoseconds per operation: the 99th percentile of batch means. */
  readonly p99: number;
  /** Nanoseconds per operation: the largest batch mean. */
  readonly max: number;
  /** Nanoseconds per operation: the mean of the batch means. */
  readonly mean: number;
  /** What was measured, in a sentence: shown in the table's notes. */
  readonly note: string;
}

/** An {@link OpResult} from the batch means (nanoseconds per operation) of one operation. */
export function opResult(id: string, mode: TimingMode, batch: number, perOpNs: readonly number[], note: string): OpResult {
  const s = summarize(perOpNs);
  return { id, mode, batch, samples: s.n, min: s.min, p50: s.p50, p90: s.p90, p99: s.p99, max: s.max, mean: s.mean, note };
}
