/*
 * The live numbers the landing page shows next to the embedded playground, and how they are
 * measured. In embed mode (`?embed=1`, inside an iframe) the page posts
 *
 *   { type: "undra-stats", changeSetsPerSec, applyP50Us, applyP99Us, timerResolutionUs }
 *
 * to its parent every 500 ms.
 *
 * WHAT THE APPLY TIME MEASURES. The runtime exposes no "a change-set was applied" callback, so
 * the number is taken around the one public seam there is: `UndraCore.shared.mirror.flush()`
 * (see `instrumentMirror`, which wraps that method on the live `Mirror` instance and leaves the
 * runtime's sources untouched). A sample is the `performance.now()` time the mirror spends in
 * one flush: decoding the entries of the change-sets that flush applies into the stores' signals
 * (the generated `_apply`) and notifying the signals' subscribers. It starts when the flush
 * starts, so the wasm call that produced the change-set and the microtask wait before the flush
 * are not in it, and it ends when the flush returns, so the React re-render the subscribers
 * schedule is not in it either. A flush that applies several change-sets counts each of them once,
 * with the flush's time divided evenly between them; since ADR-031 the mirror merges them per
 * signal first (a signal written by many change-sets is applied once per flush), so the time per
 * change-set falls as the rate rises. Change-sets of every store in the core are
 * measured, not only the ones the visible screen draws, and the loading of the stores (before the
 * first message) is not.
 *
 * Two limits to state wherever the number is shown. First, the resolution of `performance.now()`
 * is set by the browser, not by this code: about 100 microseconds in Chrome and 1 millisecond in
 * Firefox and Safari for a page that is not cross-origin isolated (5 microseconds in Chrome when it
 * is). Measured in Chrome, a flush that takes about 30 us reads 0 or 100 us, so p50 flips between
 * the two. An apply faster than the step reads as 0 or as one step of it. `timerResolutionUs` is
 * the step this page measured at start-up: show an apply time at or below it as "under <step>",
 * not as a precise value. Second, it is the mirror's apply time, not the latency of a write: it
 * says nothing about the core call, which the page does not time.
 *
 * `changeSetsPerSec` is the number of change-sets the mirror applied in the last two seconds
 * (or since the stats started, when that is shorter) divided by that span; it can be fractional.
 * `applyP50Us` and `applyP99Us` are nearest-rank percentiles, in microseconds, of the samples of
 * the last two seconds (at most the most recent 50,000 flushes). All three are 0 while the window
 * holds no change-set.
 */

/** The message the page posts to its parent: what the landing page's counters read. */
export interface StatsMessage {
  readonly type: "undra-stats";
  /** Change-sets applied per second, over the last two seconds. May be fractional. */
  readonly changeSetsPerSec: number;
  /** Median apply time of one change-set, in microseconds; 0 when there are no samples. */
  readonly applyP50Us: number;
  /** 99th-percentile apply time of one change-set, in microseconds; 0 when there are no samples. */
  readonly applyP99Us: number;
  /**
   * The smallest step of this browser's `performance.now()`, in microseconds, observed when the
   * stats started: apply times at or below it are not resolved. 0 when no step could be observed.
   */
  readonly timerResolutionUs: number;
}

/** The numbers a {@link StatsWindow} computes: a {@link StatsMessage} without its type and timer resolution. */
export type StatsSnapshot = Omit<StatsMessage, "type" | "timerResolutionUs">;

/** Options of a {@link StatsWindow}. */
export interface StatsWindowOptions {
  /** The clock, in milliseconds (`performance.now` in the page; a fake one in tests). */
  readonly now: () => number;
  /** How far back the window reaches, in milliseconds. Default 2000. */
  readonly windowMs?: number;
  /** Flushes kept at most; the oldest are dropped first. Default 50,000. */
  readonly maxSamples?: number;
}

/** One flush: when it ended, how long it took, and how many change-sets it applied. */
interface Sample {
  readonly at: number;
  readonly us: number;
  readonly count: number;
}

const ZERO: StatsSnapshot = { changeSetsPerSec: 0, applyP50Us: 0, applyP99Us: 0 };

/**
 * A sliding window over the apply times of change-sets, with an injected clock so it is exact and
 * deterministic in tests. Pure: no DOM, no timers, no runtime.
 *
 * ```ts
 * const stats = new StatsWindow({ now: () => performance.now() });
 * stats.record(120);          // one change-set took 120 us to apply
 * stats.snapshot();           // { changeSetsPerSec, applyP50Us, applyP99Us }
 * ```
 */
export class StatsWindow {
  readonly #now: () => number;
  readonly #windowMs: number;
  readonly #maxSamples: number;
  readonly #startedAt: number;
  #samples: Sample[] = [];
  /** Index of the oldest live sample in `#samples`; earlier ones are expired and dropped lazily. */
  #head = 0;

  /** @param options See {@link StatsWindowOptions}. The window starts now. */
  constructor(options: StatsWindowOptions) {
    this.#now = options.now;
    this.#windowMs = options.windowMs ?? 2000;
    this.#maxSamples = options.maxSamples ?? 50_000;
    this.#startedAt = options.now();
  }

  /**
   * Records that `changeSets` change-sets (default 1) were applied in `durationUs` microseconds
   * in total, now. Negative or non-finite durations and counts below one are ignored.
   */
  record(durationUs: number, changeSets = 1): void {
    if (!Number.isFinite(durationUs) || durationUs < 0 || !Number.isFinite(changeSets) || changeSets < 1) return;
    const now = this.#now();
    this.#expire(now);
    const count = Math.floor(changeSets);
    this.#samples.push({ at: now, us: durationUs / count, count });
    if (this.#samples.length - this.#head > this.#maxSamples) this.#head++;
  }

  /** The rate and percentiles over the window that ends now. All zero when it holds no change-set. */
  snapshot(): StatsSnapshot {
    const now = this.#now();
    this.#expire(now);
    const live = this.#samples.slice(this.#head);
    if (live.length === 0) return ZERO;
    let total = 0;
    for (const sample of live) total += sample.count;
    // The window is shorter than `windowMs` until the stats have run that long.
    const spanMs = Math.min(this.#windowMs, now - this.#startedAt);
    live.sort((a, b) => a.us - b.us);
    return {
      changeSetsPerSec: spanMs > 0 ? (total * 1000) / spanMs : 0,
      applyP50Us: percentile(live, total, 0.5),
      applyP99Us: percentile(live, total, 0.99),
    };
  }

  /** Drops what left the window `(now - windowMs, now]`. */
  #expire(now: number): void {
    const oldest = now - this.#windowMs;
    while (this.#head < this.#samples.length && (this.#samples[this.#head] as Sample).at <= oldest) this.#head++;
    // Reclaim the dropped prefix once it is the larger part of the array.
    if (this.#head > 1024 && this.#head * 2 > this.#samples.length) {
      this.#samples = this.#samples.slice(this.#head);
      this.#head = 0;
    }
  }
}

/**
 * The nearest-rank percentile `p` (0..1) of `sorted` samples, each standing for `count`
 * change-sets of `us` microseconds: the value of the change-set at rank `ceil(p * total)`.
 */
function percentile(sorted: readonly Sample[], total: number, p: number): number {
  const rank = Math.max(1, Math.ceil(p * total));
  let seen = 0;
  for (const sample of sorted) {
    seen += sample.count;
    if (seen >= rank) return sample.us;
  }
  return (sorted[sorted.length - 1] as Sample).us;
}

/** Rounds to a tenth, so a clock's floating-point residue (`100.00000000000142`) does not reach the page. */
const tenth = (value: number): number => Math.round(value * 10) / 10;

/** The message for a snapshot and the clock's resolution: numbers rounded to a tenth. */
export function toStatsMessage(snapshot: StatsSnapshot, timerResolutionUs: number): StatsMessage {
  return {
    type: "undra-stats",
    changeSetsPerSec: tenth(snapshot.changeSetsPerSec),
    applyP50Us: tenth(snapshot.applyP50Us),
    applyP99Us: tenth(snapshot.applyP99Us),
    timerResolutionUs: tenth(timerResolutionUs),
  };
}

/** Consecutive readings of the clock that {@link timerResolutionUs} spins through, at most. */
const MAX_SPIN_READS = 500_000;

/**
 * The smallest step of `now()` (milliseconds), in microseconds, seen over a few consecutive
 * changes of its reading: what the browser's clock clamp leaves of `performance.now()`. Spins for
 * a fraction of a millisecond in Chrome and a few in Firefox and Safari, and gives up after
 * {@link MAX_SPIN_READS} readings; returns 0 when it saw no step (a frozen clock).
 */
export function timerResolutionUs(now: () => number = () => performance.now(), steps = 3): number {
  let smallest = Number.POSITIVE_INFINITY;
  let last = now();
  for (let reads = 0, seen = 0; seen < steps && reads < MAX_SPIN_READS; reads++) {
    const t = now();
    if (t > last) {
      smallest = Math.min(smallest, t - last);
      last = t;
      seen++;
    }
  }
  return Number.isFinite(smallest) ? smallest * 1000 : 0;
}

/** The part of the runtime's `Mirror` that {@link instrumentMirror} uses. */
export interface MirrorLike {
  /** Entries waiting for the next flush. */
  readonly pending: number;
  /** Accepts a change-set payload and queues its entries. */
  enqueue(payload: Uint8Array): void;
  /** Applies every queued entry and notifies the signals' subscribers. */
  flush(): void;
}

/**
 * Times every flush of `mirror` with `now()` (milliseconds) and reports it to `onApplied` as
 * `(durationUs, changeSets)`: the microseconds the flush took and how many change-sets it
 * applied. A nested `flush()` (the mirror ignores it) and a flush with nothing queued are not
 * reported; change-sets with no entries apply nothing and are not counted.
 *
 * This wraps the `enqueue` and `flush` methods of that one instance, which is how app code can
 * observe the mirror without changing the runtime; the mirror looks `flush` up on itself every
 * time, so the wrapper sees scheduled flushes and the direct ones `observe` makes alike. Returns
 * a function that puts the original methods back.
 */
export function instrumentMirror(
  mirror: MirrorLike,
  onApplied: (durationUs: number, changeSets: number) => void,
  now: () => number = () => performance.now(),
): () => void {
  const enqueue = mirror.enqueue;
  const flush = mirror.flush;
  /** Change-sets queued and not yet applied. */
  let queued = 0;
  let flushing = false;

  mirror.enqueue = (payload) => {
    const before = mirror.pending;
    enqueue.call(mirror, payload);
    if (mirror.pending > before) queued++;
  };
  mirror.flush = () => {
    if (flushing || mirror.pending === 0) {
      flush.call(mirror);
      return;
    }
    flushing = true;
    const started = now();
    try {
      flush.call(mirror);
    } finally {
      const elapsedMs = now() - started;
      flushing = false;
      const applied = queued;
      queued = 0;
      if (applied > 0) onApplied(elapsedMs * 1000, applied);
    }
  };
  return () => {
    mirror.enqueue = enqueue;
    mirror.flush = flush;
  };
}

/** Where the page posts its stats: the parent window. */
export interface MessageTarget {
  postMessage(message: unknown, targetOrigin: string): void;
}

/** How often the stats are posted. */
export const STATS_INTERVAL_MS = 500;

/**
 * Starts measuring `mirror` and posts a {@link StatsMessage} to `parent` every
 * {@link STATS_INTERVAL_MS}, addressed to `targetOrigin` (the embedding page's origin; the
 * landing page is served from this page's own origin, so `location.origin` is exact and a frame
 * from anywhere else never receives the message). Call it once the stores are created, so their
 * loading is not measured. Returns a function that stops both the posting and the measuring.
 */
export function startStatsPoster(
  mirror: MirrorLike,
  parent: MessageTarget,
  now: () => number = () => performance.now(),
  targetOrigin = "*",
): () => void {
  const resolutionUs = timerResolutionUs(now);
  const stats = new StatsWindow({ now });
  const uninstall = instrumentMirror(mirror, (us, changeSets) => stats.record(us, changeSets), now);
  const timer = setInterval(() => {
    parent.postMessage(toStatsMessage(stats.snapshot(), resolutionUs), targetOrigin);
  }, STATS_INTERVAL_MS);
  return () => {
    clearInterval(timer);
    uninstall();
  };
}
