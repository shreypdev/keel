import type { ClockAdapter } from "@undra/runtime";

/** Where the manual clock starts (scenarios.md, the harness): 14 November 2023, in milliseconds since the epoch. */
export const CLOCK_START_MS = 1_700_000_000_000;

/**
 * The `Clock` port of the contract tests: time stands still until the test moves it.
 *
 * The core's query cache reads it (`now_ms`) to decide whether data is stale; timers are not the
 * clock's, they stay real (`setTimeout`).
 *
 * ```ts
 * const clock = new ManualClock();
 * clock.advance(31_000); // 31 seconds later, instantly
 * ```
 */
export class ManualClock implements ClockAdapter {
  #now: number;

  /** @param startMs The first reading, in milliseconds since the Unix epoch. */
  constructor(startMs: number = CLOCK_START_MS) {
    this.#now = startMs;
  }

  /** The current reading, in milliseconds since the Unix epoch. */
  nowMs(): number {
    return this.#now;
  }

  /** The same instant as nanoseconds, so the monotonic clock never runs backwards while the test only moves forward. */
  monotonicNs(): bigint {
    return BigInt(this.#now) * 1_000_000n;
  }

  /** Sets the clock to `ms` milliseconds since the Unix epoch. */
  set(ms: number): void {
    this.#now = ms;
  }

  /** Moves the clock forward by `ms` milliseconds. */
  advance(ms: number): void {
    this.#now += ms;
  }
}
