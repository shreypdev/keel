import { ChangeOp, Mirror, encodeChangeSet } from "@keel/runtime";
import { afterEach, describe, expect, test, vi } from "vitest";
import { STATS_INTERVAL_MS, StatsWindow, instrumentMirror, startStatsPoster, timerResolutionUs, toStatsMessage } from "./embed-stats";

/** A clock the test moves by hand, in milliseconds. */
function fakeClock(start = 0): { now: () => number; advance: (ms: number) => void } {
  let t = start;
  return {
    now: () => t,
    advance: (ms) => {
      t += ms;
    },
  };
}

describe("StatsWindow", () => {
  test("no samples: everything is 0", () => {
    const clock = fakeClock();
    const stats = new StatsWindow({ now: clock.now });
    expect(stats.snapshot()).toEqual({ changeSetsPerSec: 0, applyP50Us: 0, applyP99Us: 0 });
    clock.advance(5000);
    expect(stats.snapshot()).toEqual({ changeSetsPerSec: 0, applyP50Us: 0, applyP99Us: 0 });
  });

  test("a steady 10 per second reads 10 per second", () => {
    const clock = fakeClock();
    const stats = new StatsWindow({ now: clock.now });
    for (let i = 0; i < 50; i++) {
      clock.advance(100);
      stats.record(100);
    }
    expect(stats.snapshot().changeSetsPerSec).toBeCloseTo(10, 10);
  });

  test("the rate uses the time since the start while that is shorter than the window", () => {
    const clock = fakeClock(1000);
    const stats = new StatsWindow({ now: clock.now });
    for (let i = 0; i < 5; i++) {
      clock.advance(100);
      stats.record(50);
    }
    // 5 change-sets in the 500 ms since the start: 10 per second, not 5 / 2 s.
    expect(stats.snapshot().changeSetsPerSec).toBeCloseTo(10, 10);
  });

  test("nothing is reported at the instant the window starts", () => {
    const clock = fakeClock(42);
    const stats = new StatsWindow({ now: clock.now });
    stats.record(10);
    expect(stats.snapshot().changeSetsPerSec).toBe(0);
  });

  test("the rate can be fractional", () => {
    const clock = fakeClock();
    const stats = new StatsWindow({ now: clock.now });
    clock.advance(1500);
    stats.record(10);
    stats.record(10);
    expect(stats.snapshot().changeSetsPerSec).toBeCloseTo(2000 / 1500, 10);
  });

  test("percentiles are nearest-rank over the samples", () => {
    const clock = fakeClock();
    const stats = new StatsWindow({ now: clock.now });
    clock.advance(1000);
    // 100 samples: 1..100 microseconds, recorded out of order.
    for (let i = 0; i < 100; i++) stats.record(((i * 37) % 100) + 1);
    const snapshot = stats.snapshot();
    expect(snapshot.applyP50Us).toBe(50);
    expect(snapshot.applyP99Us).toBe(99);
  });

  test("one sample is every percentile", () => {
    const clock = fakeClock();
    const stats = new StatsWindow({ now: clock.now });
    clock.advance(10);
    stats.record(123.5);
    expect(stats.snapshot()).toMatchObject({ applyP50Us: 123.5, applyP99Us: 123.5 });
  });

  test("the slowest 1% is the p99, not the median", () => {
    const clock = fakeClock();
    const stats = new StatsWindow({ now: clock.now });
    clock.advance(1000);
    for (let i = 0; i < 98; i++) stats.record(20);
    stats.record(900);
    stats.record(5000);
    const snapshot = stats.snapshot();
    expect(snapshot.applyP50Us).toBe(20);
    expect(snapshot.applyP99Us).toBe(900);
  });

  test("a flush of several change-sets is that many samples of the average", () => {
    const clock = fakeClock();
    const stats = new StatsWindow({ now: clock.now });
    clock.advance(1000);
    stats.record(400, 4); // four change-sets in 400 us: 100 us each
    stats.record(10, 1);
    const snapshot = stats.snapshot();
    expect(snapshot.changeSetsPerSec).toBeCloseTo(5, 10);
    expect(snapshot.applyP50Us).toBe(100);
    expect(snapshot.applyP99Us).toBe(100);
  });

  test("samples leave the window after two seconds, and the numbers go back to 0", () => {
    const clock = fakeClock();
    const stats = new StatsWindow({ now: clock.now });
    clock.advance(1000);
    stats.record(900);
    clock.advance(1500);
    stats.record(100);
    // The 900 us sample is 1.5 s old, still inside.
    expect(stats.snapshot().applyP99Us).toBe(900);
    clock.advance(600);
    // Now it is 2.1 s old, the 100 us one 0.6 s.
    expect(stats.snapshot().applyP99Us).toBe(100);
    clock.advance(1500);
    expect(stats.snapshot()).toEqual({ changeSetsPerSec: 0, applyP50Us: 0, applyP99Us: 0 });
  });

  test("the window is (now - windowMs, now]: a sample exactly windowMs old is out", () => {
    const clock = fakeClock();
    const stats = new StatsWindow({ now: clock.now, windowMs: 1000 });
    clock.advance(10);
    stats.record(7);
    clock.advance(999);
    expect(stats.snapshot().applyP50Us).toBe(7);
    clock.advance(1);
    expect(stats.snapshot().applyP50Us).toBe(0);
  });

  test("a steady stream at the rate keeps a steady window", () => {
    const clock = fakeClock();
    const stats = new StatsWindow({ now: clock.now });
    for (let i = 0; i < 1000; i++) {
      clock.advance(100);
      stats.record(i % 2 === 0 ? 30 : 60);
    }
    const snapshot = stats.snapshot();
    expect(snapshot.changeSetsPerSec).toBeCloseTo(10, 10);
    expect(snapshot.applyP50Us).toBe(30);
    expect(snapshot.applyP99Us).toBe(60);
  });

  test("at most maxSamples flushes are kept, the oldest dropped first", () => {
    const clock = fakeClock();
    const stats = new StatsWindow({ now: clock.now, maxSamples: 3 });
    clock.advance(100);
    for (const us of [1000, 1, 2, 3]) stats.record(us);
    // The 1000 us sample was dropped; 1, 2, 3 remain.
    expect(stats.snapshot().applyP99Us).toBe(3);
  });

  test("a long run reclaims expired samples and stays correct", () => {
    const clock = fakeClock();
    const stats = new StatsWindow({ now: clock.now });
    for (let i = 0; i < 20_000; i++) {
      clock.advance(1);
      stats.record(5);
    }
    const snapshot = stats.snapshot();
    // 1 ms spacing for 2 s: 2000 change-sets in the window.
    expect(snapshot.changeSetsPerSec).toBeCloseTo(1000, 6);
    expect(snapshot.applyP50Us).toBe(5);
  });

  test("ignores durations and counts that are not usable", () => {
    const clock = fakeClock();
    const stats = new StatsWindow({ now: clock.now });
    clock.advance(1000);
    stats.record(Number.NaN);
    stats.record(Number.POSITIVE_INFINITY);
    stats.record(-1);
    stats.record(10, 0);
    stats.record(10, -2);
    stats.record(10, Number.NaN);
    expect(stats.snapshot()).toEqual({ changeSetsPerSec: 0, applyP50Us: 0, applyP99Us: 0 });
    stats.record(10, 2.9);
    expect(stats.snapshot().changeSetsPerSec).toBeCloseTo(2, 10);
  });
});

describe("toStatsMessage", () => {
  test("types the message and rounds to a tenth", () => {
    expect(toStatsMessage({ changeSetsPerSec: 9.987654, applyP50Us: 100.00000000000142, applyP99Us: 249.96 }, 99.99999999999)).toEqual({
      type: "keel-stats",
      changeSetsPerSec: 10,
      applyP50Us: 100,
      applyP99Us: 250,
      timerResolutionUs: 100,
    });
    expect(toStatsMessage({ changeSetsPerSec: 0, applyP50Us: 0, applyP99Us: 0 }, 0)).toEqual({
      type: "keel-stats",
      changeSetsPerSec: 0,
      applyP50Us: 0,
      applyP99Us: 0,
      timerResolutionUs: 0,
    });
  });
});

describe("timerResolutionUs", () => {
  /** A clock that advances `tickMs` per reading and shows only multiples of `stepMs`, like a clamped performance.now. */
  function clamped(stepMs: number, tickMs: number): () => number {
    let t = 0;
    return () => {
      t += tickMs;
      return Math.floor(t / stepMs) * stepMs;
    };
  }

  test("finds the step of a clamped clock", () => {
    expect(timerResolutionUs(clamped(0.1, 0.001))).toBeCloseTo(100, 6);
    expect(timerResolutionUs(clamped(1, 0.001))).toBeCloseTo(1000, 6);
    expect(timerResolutionUs(clamped(0.005, 0.0001))).toBeCloseTo(5, 6);
  });

  test("takes the smallest step it sees", () => {
    const readings = [0, 0, 0.3, 0.3, 0.4, 0.4, 0.4, 0.9];
    let i = 0;
    expect(timerResolutionUs(() => readings[Math.min(i++, readings.length - 1)] as number)).toBeCloseTo(100, 6);
  });

  test("a frozen clock has no step to find, and the spin ends", () => {
    expect(timerResolutionUs(() => 5)).toBe(0);
  });

  test("the real clock gives a positive, sane step", () => {
    const step = timerResolutionUs();
    expect(step).toBeGreaterThan(0);
    expect(step).toBeLessThan(5000);
  });
});

describe("instrumentMirror, on the runtime's real Mirror", () => {
  const STORE = 7n;

  /** A change-set with one full-value entry for the store. */
  const changeSet = (txnId: number): Uint8Array =>
    encodeChangeSet({ txnId: BigInt(txnId), entries: [{ handle: STORE, signalId: 0, op: ChangeOp.FullValue, value: new Uint8Array([1]) }] });

  /** A mirror whose flushes the test runs by hand, whose store applies in `applyMs` of fake time. */
  function setup(applyMs: number) {
    const clock = fakeClock();
    const scheduled: Array<() => void> = [];
    const errors: unknown[] = [];
    const mirror = new Mirror({ schedule: (fn) => scheduled.push(fn), onError: (error) => errors.push(error) });
    const applied: Array<{ us: number; changeSets: number }> = [];
    let entries = 0;
    mirror.register(STORE, () => {
      entries++;
      clock.advance(applyMs);
    });
    const restore = instrumentMirror(mirror, (us, changeSets) => applied.push({ us, changeSets }), clock.now);
    const runScheduled = (): void => {
      while (scheduled.length > 0) (scheduled.shift() as () => void)();
    };
    return { clock, mirror, applied, errors, restore, runScheduled, entries: () => entries };
  }

  test("reports the time of a scheduled flush, in microseconds, for one change-set", () => {
    const t = setup(0.25);
    t.mirror.enqueue(changeSet(1));
    // Time spent waiting for the flush is not part of it.
    t.clock.advance(3);
    t.runScheduled();
    expect(t.entries()).toBe(1);
    expect(t.applied).toHaveLength(1);
    expect(t.applied[0]).toEqual({ us: 250, changeSets: 1 });
    expect(t.errors).toEqual([]);
  });

  test("change-sets queued before one flush are applied, and counted, by that flush", () => {
    const t = setup(0.1);
    t.mirror.enqueue(changeSet(1));
    t.mirror.enqueue(changeSet(2));
    t.mirror.enqueue(changeSet(3));
    t.runScheduled();
    expect(t.entries()).toBe(3);
    expect(t.applied).toHaveLength(1);
    expect(t.applied[0]?.changeSets).toBe(3);
    expect(t.applied[0]?.us).toBeCloseTo(300, 6);
  });

  test("a direct flush() (what observe does) is measured like a scheduled one", () => {
    const t = setup(0.5);
    t.mirror.enqueue(changeSet(1));
    t.mirror.flush();
    expect(t.applied).toEqual([{ us: 500, changeSets: 1 }]);
    // The scheduled flush that enqueue asked for finds nothing to do and reports nothing.
    t.runScheduled();
    expect(t.applied).toHaveLength(1);
  });

  test("a flush with nothing queued, and a change-set with no entries, are not reported", () => {
    const t = setup(0.5);
    t.mirror.flush();
    t.mirror.enqueue(encodeChangeSet({ txnId: 1n, entries: [] }));
    t.runScheduled();
    expect(t.applied).toEqual([]);
    // It was accepted by the mirror, but it applied nothing.
    expect(t.mirror.changeSets).toBe(1);
  });

  test("a rejected payload is reported by the mirror, not counted", () => {
    const t = setup(0.5);
    t.mirror.enqueue(new Uint8Array([1, 2, 3]));
    t.runScheduled();
    expect(t.errors).toHaveLength(1);
    expect(t.applied).toEqual([]);
  });

  test("entries for a store nobody registered apply nothing but the change-set is still one flush", () => {
    const t = setup(0.5);
    t.mirror.enqueue(encodeChangeSet({ txnId: 1n, entries: [{ handle: 99n, signalId: 0, op: ChangeOp.FullValue, value: new Uint8Array() }] }));
    t.runScheduled();
    expect(t.mirror.dropped).toBe(1);
    expect(t.applied).toEqual([{ us: 0, changeSets: 1 }]);
  });

  test("a nested flush() from inside an apply is neither measured twice nor lost", () => {
    const clock = fakeClock();
    const scheduled: Array<() => void> = [];
    const mirror = new Mirror({ schedule: (fn) => scheduled.push(fn) });
    const applied: Array<{ us: number; changeSets: number }> = [];
    mirror.register(STORE, () => {
      clock.advance(0.2);
      mirror.flush();
    });
    instrumentMirror(mirror, (us, changeSets) => applied.push({ us, changeSets }), clock.now);
    mirror.enqueue(changeSet(1));
    (scheduled.shift() as () => void)();
    expect(applied).toHaveLength(1);
    expect(applied[0]?.us).toBeCloseTo(200, 6);
  });

  test("a change-set that arrives while a flush runs is applied by it and counted with it", () => {
    const clock = fakeClock();
    const scheduled: Array<() => void> = [];
    const mirror = new Mirror({ schedule: (fn) => scheduled.push(fn) });
    const applied: Array<{ us: number; changeSets: number }> = [];
    let first = true;
    mirror.register(STORE, () => {
      clock.advance(0.1);
      if (first) {
        first = false;
        // What a subscriber that makes a synchronous core call does.
        mirror.enqueue(changeSet(2));
      }
    });
    instrumentMirror(mirror, (us, changeSets) => applied.push({ us, changeSets }), clock.now);
    mirror.enqueue(changeSet(1));
    (scheduled.shift() as () => void)();
    expect(applied).toHaveLength(1);
    expect(applied[0]?.changeSets).toBe(2);
    expect(applied[0]?.us).toBeCloseTo(200, 6);
  });

  test("a store that throws is reported by the mirror and the flush is still measured", () => {
    const clock = fakeClock();
    const scheduled: Array<() => void> = [];
    const errors: unknown[] = [];
    const mirror = new Mirror({ schedule: (fn) => scheduled.push(fn), onError: (error) => errors.push(error) });
    const applied: Array<{ us: number; changeSets: number }> = [];
    mirror.register(STORE, () => {
      clock.advance(0.3);
      throw new Error("boom");
    });
    instrumentMirror(mirror, (us, changeSets) => applied.push({ us, changeSets }), clock.now);
    mirror.enqueue(changeSet(1));
    (scheduled.shift() as () => void)();
    expect(errors).toHaveLength(1);
    expect(applied).toHaveLength(1);
    expect(applied[0]?.us).toBeCloseTo(300, 6);
    expect(applied[0]?.changeSets).toBe(1);
  });

  test("the restore function puts the original methods back", () => {
    const t = setup(0.5);
    t.restore();
    t.mirror.enqueue(changeSet(1));
    t.runScheduled();
    expect(t.entries()).toBe(1);
    expect(t.applied).toEqual([]);
  });

  test("feeds a StatsWindow: the numbers a counter would show", () => {
    const t = setup(0.1);
    const stats = new StatsWindow({ now: t.clock.now });
    t.restore();
    instrumentMirror(t.mirror, (us, changeSets) => stats.record(us, changeSets), t.clock.now);
    for (let i = 0; i < 20; i++) {
      t.clock.advance(100);
      t.mirror.enqueue(changeSet(i));
      t.runScheduled();
    }
    const snapshot = stats.snapshot();
    // 20 change-sets in a window that is 2 s long by now.
    expect(snapshot.changeSetsPerSec).toBeCloseTo(10, 10);
    expect(snapshot.applyP50Us).toBeCloseTo(100, 6);
    expect(snapshot.applyP99Us).toBeCloseTo(100, 6);
  });
});

describe("startStatsPoster", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  test("posts a keel-stats message to the parent every 500 ms, zeros while idle", () => {
    vi.useFakeTimers();
    const clock = fakeClock();
    const mirror = new Mirror({ schedule: () => {} });
    const posted: Array<{ message: unknown; origin: string }> = [];
    const stop = startStatsPoster(mirror, { postMessage: (message, origin) => posted.push({ message, origin }) }, clock.now);
    expect(STATS_INTERVAL_MS).toBe(500);

    vi.advanceTimersByTime(499);
    expect(posted).toEqual([]);
    vi.advanceTimersByTime(1);
    // The fake clock never moves by itself, so no timer step can be observed.
    expect(posted).toEqual([{ message: { type: "keel-stats", changeSetsPerSec: 0, applyP50Us: 0, applyP99Us: 0, timerResolutionUs: 0 }, origin: "*" }]);

    vi.advanceTimersByTime(1000);
    expect(posted).toHaveLength(3);

    stop();
    vi.advanceTimersByTime(5000);
    expect(posted).toHaveLength(3);
  });

  test("the messages carry the measured numbers", () => {
    vi.useFakeTimers();
    const clock = fakeClock();
    const scheduled: Array<() => void> = [];
    const mirror = new Mirror({ schedule: (fn) => scheduled.push(fn) });
    mirror.register(9n, () => {
      clock.advance(0.2);
    });
    const posted: unknown[] = [];
    const stop = startStatsPoster(mirror, { postMessage: (message) => posted.push(message) }, clock.now);

    // Five change-sets in the 500 ms before the first message, 200 us each.
    for (let i = 0; i < 5; i++) {
      clock.advance(100);
      mirror.enqueue(encodeChangeSet({ txnId: BigInt(i), entries: [{ handle: 9n, signalId: 0, op: ChangeOp.FullValue, value: new Uint8Array([0]) }] }));
      (scheduled.shift() as () => void)();
    }
    vi.advanceTimersByTime(500);
    expect(posted).toHaveLength(1);
    const message = posted[0] as { type: string; changeSetsPerSec: number; applyP50Us: number; applyP99Us: number; timerResolutionUs: number };
    expect(message.type).toBe("keel-stats");
    expect(message.applyP50Us).toBeCloseTo(200, 6);
    expect(message.applyP99Us).toBeCloseTo(200, 6);
    // The fake clock advanced 5 * (100 + 0.2) ms = 501 ms; 5 change-sets in that span is about 10 per second.
    expect(message.changeSetsPerSec).toBeGreaterThan(9.9);
    expect(message.changeSetsPerSec).toBeLessThan(10.1);
    stop();
  });
});
