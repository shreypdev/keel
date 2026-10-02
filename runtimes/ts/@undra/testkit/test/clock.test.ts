import { describe, expect, it } from "vitest";
import { FakeClock, TimerStormError } from "../src/index.js";

describe("the manual clock", () => {
  it("fires due timers in deadline order, ties in arming order, and a timer armed inside the window fires in the same call", () => {
    const clock = new FakeClock(0);
    const order: number[] = [];
    clock.set(3, 20, (id) => order.push(id));
    clock.set(1, 10, (id) => {
      order.push(id);
      clock.set(4, 5, (again) => order.push(again)); // due at 15, inside the window
    });
    clock.set(2, 10, (id) => order.push(id));
    expect(clock.advance(30)).toEqual([1, 2, 4, 3]);
    expect(order).toEqual([1, 2, 4, 3]);
    expect(clock.monotonicNs()).toBe(30_000_000n);
  });

  it("a timer that re-arms itself without time passing stops at the cap with a typed error, and the clock stays where it was", () => {
    const clock = new FakeClock(0);
    const again = (id: number): void => clock.set(id, 0, again);
    clock.set(7, 0, again);
    let error: unknown;
    try {
      clock.advance(1_000, 50);
    } catch (e) {
      error = e;
    }
    expect(error).toBeInstanceOf(TimerStormError);
    expect(error).toMatchObject({ fired: 50, timerId: 7, atMs: 0 });
    expect((error as Error).message).toMatch(/timer 7/);
    expect(clock.monotonicNs()).toBe(0n);
    expect(clock.pendingTimers).toBe(1);
  });

  it("a periodic timer is fine until its window holds more firings than the cap", () => {
    const clock = new FakeClock(0);
    let count = 0;
    const tick = (id: number): void => {
      count += 1;
      clock.set(id, 1, tick);
    };
    clock.set(1, 1, tick);
    expect(clock.advance(100)).toHaveLength(100);
    expect(() => clock.advance(10_000, 500)).toThrow(TimerStormError);
    expect(count).toBe(600);
  });
});
