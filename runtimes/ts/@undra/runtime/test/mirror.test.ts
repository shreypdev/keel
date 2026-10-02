import { describe, expect, it, vi } from "vitest";
import { mirrorWaiters } from "../src/mirror-waiters.js";
import { Mirror } from "../src/mirror.js";
import { Signal, setSignalErrorHandler } from "../src/signal.js";
import { ALL_SIGNALS, ChangeOp, encodeChangeSet } from "../src/wire/index.js";
import { macrotask } from "./support/harness.js";

const bytes = (...v: number[]) => Uint8Array.from(v);

function changeSet(...entries: Array<[bigint, number, Uint8Array?, ChangeOp?]>): Uint8Array {
  return encodeChangeSet({
    txnId: 1n,
    entries: entries.map(([handle, signalId, value = new Uint8Array(0), op = ChangeOp.FullValue]) => ({
      handle,
      signalId,
      op,
      value,
    })),
  });
}

describe("Mirror registry", () => {
  it("registers, finds and unregisters handles", () => {
    const mirror = new Mirror();
    mirror.register(1n, () => {});
    expect([mirror.has(1n), mirror.has(2n), mirror.size]).toEqual([true, false, 1]);
    mirror.unregister(1n);
    mirror.unregister(1n);
    expect([mirror.has(1n), mirror.size]).toEqual([false, 0]);
  });

  it("refuses to register a handle twice", () => {
    const mirror = new Mirror();
    mirror.register(1n, () => {});
    expect(() => mirror.register(1n, () => {})).toThrow(/already registered/);
  });

  it("keys handles by value, including large generations", () => {
    const mirror = new Mirror();
    const apply = vi.fn();
    mirror.register(0xffff_ffff_0000_0001n, apply);
    mirror.enqueue(changeSet([0xffff_ffff_0000_0001n, 0]));
    mirror.flush();
    expect(apply).toHaveBeenCalledOnce();
  });
});

describe("Mirror change-sets", () => {
  it("applies entries in order with signal id, op and value", () => {
    const mirror = new Mirror();
    const seen: unknown[] = [];
    mirror.register(1n, (signalId, op, value) => seen.push([signalId, op, [...value]]));
    mirror.register(2n, (signalId, op, value) => seen.push(["two", signalId, op, [...value]]));
    mirror.enqueue(
      changeSet([1n, 0, bytes(1)], [2n, 5, bytes(1, 0, 0, 0, 1, 2, 0, 0, 0), ChangeOp.KeyedPatch], [1n, 1, bytes(), ChangeOp.LazyInvalidated]),
    );
    expect(seen).toEqual([]); // nothing until the flush
    expect(mirror.pending).toBe(3);
    mirror.flush();
    expect(seen).toEqual([
      [0, ChangeOp.FullValue, [1]],
      ["two", 5, ChangeOp.KeyedPatch, [1, 0, 0, 0, 1, 2, 0, 0, 0]],
      [1, ChangeOp.LazyInvalidated, []],
    ]);
    expect(mirror.pending).toBe(0);
    expect(mirror.changeSets).toBe(1);
  });

  it("schedules exactly one flush for change-sets that arrive together", () => {
    const scheduled: Array<() => void> = [];
    const mirror = new Mirror({ schedule: (fn) => scheduled.push(fn) });
    const apply = vi.fn();
    mirror.register(1n, apply);
    mirror.enqueue(changeSet([1n, 0]));
    mirror.enqueue(changeSet([1n, 1]));
    mirror.enqueue(changeSet([1n, 2]));
    expect(scheduled).toHaveLength(1);
    scheduled[0]?.();
    expect(apply.mock.calls.map((c) => c[0])).toEqual([0, 1, 2]);
    mirror.enqueue(changeSet([1n, 3]));
    expect(scheduled).toHaveLength(2);
  });

  it("flushes on a microtask by default, once per macrotask", async () => {
    const mirror = new Mirror();
    const apply = vi.fn();
    mirror.register(1n, apply);
    mirror.enqueue(changeSet([1n, 0]));
    mirror.enqueue(changeSet([1n, 1]));
    expect(apply).not.toHaveBeenCalled();
    await Promise.resolve();
    expect(apply).toHaveBeenCalledTimes(2);
  });

  it("the default scheduler works where queueMicrotask insists on a global receiver (browsers)", async () => {
    // Browsers throw "Illegal invocation" when a global function such as queueMicrotask is called
    // with another object as `this`; Node does not, so model the browser.
    const real = globalThis.queueMicrotask;
    vi.stubGlobal("queueMicrotask", function (this: unknown, fn: () => void) {
      if (this !== undefined && this !== globalThis) throw new TypeError("Illegal invocation");
      real(fn);
    });
    try {
      const mirror = new Mirror();
      const apply = vi.fn();
      mirror.register(1n, apply);
      mirror.enqueue(changeSet([1n, 0]));
      await Promise.resolve();
      expect(apply).toHaveBeenCalledTimes(1);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("notifies a signal once per flush no matter how many change-sets touched it", async () => {
    const mirror = new Mirror();
    const count = new Signal(0);
    const seen: number[] = [];
    count.subscribe((v) => seen.push(v));
    mirror.register(1n, (_id, _op, value) => count._set(value[0] as number));
    for (let i = 1; i <= 5; i++) mirror.enqueue(changeSet([1n, 0, bytes(i)]));
    await macrotask();
    expect(seen).toEqual([5]);
    expect(count.peek()).toBe(5);
  });

  it("announces the signals of several stores after all of a flush is applied", () => {
    const mirror = new Mirror();
    const a = new Signal(0);
    const b = new Signal(0);
    const observed: string[] = [];
    a.subscribe(() => observed.push(`a sees b=${b.peek()}`));
    b.subscribe(() => observed.push(`b sees a=${a.peek()}`));
    mirror.register(1n, (_i, _o, v) => a._set(v[0] as number));
    mirror.register(2n, (_i, _o, v) => b._set(v[0] as number));
    mirror.enqueue(changeSet([1n, 0, bytes(7)], [2n, 0, bytes(8)]));
    mirror.flush();
    expect(observed).toEqual(["a sees b=8", "b sees a=7"]);
  });

  it("validates a whole change-set before queuing any of it", () => {
    const errors: unknown[] = [];
    const mirror = new Mirror({ onError: (e) => errors.push(e) });
    const apply = vi.fn();
    mirror.register(1n, apply);
    const good = changeSet([1n, 0, bytes(1)], [1n, 1, bytes(2)]);
    mirror.enqueue(good.subarray(0, good.length - 1)); // truncated: the first entry is fine, the second is cut
    expect(errors).toHaveLength(1);
    expect(mirror.pending).toBe(0);
    mirror.flush();
    expect(apply).not.toHaveBeenCalled();
    expect(mirror.changeSets).toBe(0);
  });

  it("drops entries of unregistered handles and counts them", () => {
    const mirror = new Mirror();
    const apply = vi.fn();
    mirror.register(1n, apply);
    mirror.enqueue(changeSet([9n, 0], [1n, 0], [9n, 1]));
    mirror.flush();
    expect(apply).toHaveBeenCalledOnce();
    expect(mirror.dropped).toBe(2);
  });

  it("reports an apply function that throws and carries on with the rest", () => {
    const errors: unknown[] = [];
    const mirror = new Mirror({ onError: (e) => errors.push(e) });
    const boom = new Error("apply failed");
    const other = vi.fn();
    mirror.register(1n, () => {
      throw boom;
    });
    mirror.register(2n, other);
    mirror.enqueue(changeSet([1n, 0], [2n, 0]));
    mirror.flush();
    expect(errors).toEqual([boom]);
    expect(other).toHaveBeenCalledOnce();
  });

  it("an empty change-set schedules nothing", () => {
    const schedule = vi.fn();
    const mirror = new Mirror({ schedule });
    mirror.enqueue(changeSet());
    expect(schedule).not.toHaveBeenCalled();
    expect(mirror.changeSets).toBe(1);
  });

  it("applies entries enqueued while flushing in the same flush and in order", () => {
    const mirror = new Mirror();
    const order: number[] = [];
    mirror.register(1n, (signalId) => {
      order.push(signalId);
      if (signalId === 0) {
        mirror.enqueue(changeSet([1n, 10]));
        mirror.flush(); // a nested flush returns at once
        order.push(-1);
      }
    });
    mirror.enqueue(changeSet([1n, 0], [1n, 1]));
    mirror.flush();
    expect(order).toEqual([0, -1, 1, 10]);
  });

  describe("a change-set enqueued by a signal subscriber while the flush notifies", () => {
    // `flush()` applies a batch, and the subscribers of the signals it touched are notified when
    // the batch ends, still inside the flush. A subscriber that makes a core call whose change-set
    // arrives at once (every synchronous call in wasm-main) enqueues it after the round's queue ran
    // dry; it used to wait for an unrelated change-set.
    const counter = (mirror: Mirror) => {
      const count = new Signal(0);
      mirror.register(1n, (_signalId, _op, value) => count._set(value[0] as number));
      return count;
    };

    it("is applied by the same flush, on the default scheduler", async () => {
      const mirror = new Mirror();
      const count = counter(mirror);
      const seen: number[] = [];
      count.subscribe((n) => {
        seen.push(n);
        if (n === 1) mirror.enqueue(changeSet([1n, 0, bytes(2)]));
      });
      mirror.enqueue(changeSet([1n, 0, bytes(1)]));
      await macrotask();
      expect(seen).toEqual([1, 2]);
      expect(mirror.pending).toBe(0);
    });

    it("is applied before the flush returns, and needs no second scheduled flush", () => {
      const scheduled: Array<() => void> = [];
      const mirror = new Mirror({ schedule: (fn) => scheduled.push(fn) });
      const count = counter(mirror);
      const seen: number[] = [];
      count.subscribe((n) => {
        seen.push(n);
        if (n < 3) mirror.enqueue(changeSet([1n, 0, bytes(n + 1)]));
      });
      mirror.enqueue(changeSet([1n, 0, bytes(1)]));
      expect(scheduled).toHaveLength(1);
      scheduled[0]?.();
      expect(seen).toEqual([1, 2, 3]);
      expect(scheduled).toHaveLength(1);
      expect(mirror.pending).toBe(0);
    });

    it("settles a whenObserved waiter for an entry that arrived that way", async () => {
      const mirror = new Mirror();
      mirrorWaiters(mirror);
      const count = counter(mirror);
      count.subscribe((n) => {
        if (n === 1) mirror.enqueue(changeSet([1n, 5, bytes(9)]));
      });
      const waiting = mirror.whenObserved(1n, 5);
      mirror.enqueue(changeSet([1n, 0, bytes(1)]));
      mirror.flush();
      await waiting;
    });

    it("announces each round once, with the signals of that round", () => {
      const mirror = new Mirror();
      const count = counter(mirror);
      const seen: number[] = [];
      count.subscribe((n) => {
        seen.push(n);
        if (n === 1) mirror.enqueue(changeSet([1n, 0, bytes(2)], [1n, 0, bytes(3)]));
      });
      mirror.enqueue(changeSet([1n, 0, bytes(1)]));
      mirror.flush();
      expect(seen).toEqual([1, 3]);
    });

    it("stops a subscriber that feeds itself after 1000 rounds, reports it and finishes later", () => {
      const scheduled: Array<() => void> = [];
      const errors: unknown[] = [];
      const mirror = new Mirror({ schedule: (fn) => scheduled.push(fn), onError: (e) => errors.push(e) });
      const count = counter(mirror);
      let rounds = 0;
      count.subscribe((n) => {
        rounds++;
        if (rounds < 2500) mirror.enqueue(changeSet([1n, 0, bytes((n + 1) % 256)]));
      });
      mirror.enqueue(changeSet([1n, 0, bytes(1)]));
      scheduled.shift()?.();
      expect(rounds).toBe(1000);
      expect(errors).toHaveLength(1);
      expect(errors[0]).toMatchObject({ kind: "state" });
      expect(mirror.pending).toBe(1);
      // The rest is not stranded: a later flush continues and finishes.
      while (scheduled.length > 0) scheduled.shift()?.();
      expect(rounds).toBe(2500);
      expect(mirror.pending).toBe(0);
    });

    it("an error that unwinds the flush leaves the queue to a later flush", () => {
      const scheduled: Array<() => void> = [];
      const mirror = new Mirror({ schedule: (fn) => scheduled.push(fn) });
      const count = counter(mirror);
      const restore = setSignalErrorHandler((error) => {
        throw error;
      });
      try {
        count.subscribe((n) => {
          if (n === 1) {
            mirror.enqueue(changeSet([1n, 0, bytes(2)]));
            throw new Error("subscriber failed");
          }
        });
        mirror.enqueue(changeSet([1n, 0, bytes(1)]));
        expect(() => scheduled.shift()?.()).toThrow("subscriber failed");
      } finally {
        setSignalErrorHandler(restore);
      }
      expect(mirror.pending).toBe(1);
      expect(scheduled).toHaveLength(1);
      scheduled.shift()?.();
      expect(count.peek()).toBe(2);
    });
  });

  it("the default error handler rethrows from a microtask", async () => {
    const seen: unknown[] = [];
    const listener = (e: unknown) => seen.push(e);
    const others = process.listeners("uncaughtException");
    for (const l of others) process.off("uncaughtException", l);
    process.on("uncaughtException", listener);
    try {
      new Mirror().enqueue(bytes(1, 2, 3));
      await macrotask();
      expect(seen).toHaveLength(1);
    } finally {
      process.off("uncaughtException", listener);
      for (const l of others) process.on("uncaughtException", l as (e: Error) => void);
    }
  });
});

/** A mirror as a core that answers later has it: with the observe waiters of `mirror-waiters.ts` installed (ADR-057). */
function withWaiters(): Mirror {
  const mirror = new Mirror();
  mirrorWaiters(mirror);
  return mirror;
}

describe("Mirror.whenObserved", () => {
  it("rejects with UndraError('state') on a mirror whose core installed no waiters (an in-process core delivers inside observe)", async () => {
    const mirror = new Mirror();
    await expect(mirror.whenObserved(1n, 0)).rejects.toMatchObject({ kind: "state" });
    mirror.failWaiters(new Error("nothing waits")); // nothing to fail, nothing thrown
    mirror.unregister(1n);
  });

  it("resolves after an entry for the signal has been applied and announced", async () => {
    const mirror = withWaiters();
    const count = new Signal(0);
    const seen: number[] = [];
    count.subscribe((v) => seen.push(v));
    mirror.register(1n, (_i, _o, v) => count._set(v[0] as number));
    let resolved = false;
    const waiting = mirror.whenObserved(1n, 3).then(() => {
      resolved = true;
    });
    mirror.enqueue(changeSet([1n, 2, bytes(1)]));
    mirror.flush();
    await Promise.resolve();
    expect(resolved).toBe(false); // another signal
    mirror.enqueue(changeSet([1n, 3, bytes(9)]));
    mirror.flush();
    await waiting;
    expect(resolved).toBe(true);
    expect(seen).toEqual([1, 9]);
  });

  it("ALL_SIGNALS is satisfied by any entry of the handle, and only by that handle", async () => {
    const mirror = withWaiters();
    mirror.register(1n, () => {});
    mirror.register(2n, () => {});
    let resolved = false;
    const waiting = mirror.whenObserved(1n, ALL_SIGNALS).then(() => {
      resolved = true;
    });
    mirror.enqueue(changeSet([2n, 0]));
    mirror.flush();
    await Promise.resolve();
    expect(resolved).toBe(false);
    mirror.enqueue(changeSet([1n, 4]));
    mirror.flush();
    await waiting;
  });

  it("settles every waiter of a handle at once, and leaves the others", async () => {
    const mirror = withWaiters();
    mirror.register(1n, () => {});
    const a = mirror.whenObserved(1n, 0);
    const b = mirror.whenObserved(1n, 0);
    const c = mirror.whenObserved(1n, 1);
    mirror.enqueue(changeSet([1n, 0]));
    mirror.flush();
    await Promise.all([a, b]);
    mirror.unregister(1n);
    await c; // unregistering resolves the rest
  });

  it("rejects with an observe error when nothing arrives in time, and forgets the waiter", async () => {
    vi.useFakeTimers();
    try {
      const mirror = withWaiters();
      const waiting = mirror.whenObserved(7n, 0, 50);
      const assertion = expect(waiting).rejects.toMatchObject({ kind: "observe" });
      await vi.advanceTimersByTimeAsync(50);
      await assertion;
      await expect(waiting).rejects.toThrow(/handle 7/);
      // A late entry is dropped without trouble.
      mirror.enqueue(changeSet([7n, 0]));
      mirror.flush();
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not time out once resolved", async () => {
    vi.useFakeTimers();
    try {
      const mirror = withWaiters();
      mirror.register(1n, () => {});
      const waiting = mirror.whenObserved(1n, 0, 50);
      mirror.enqueue(changeSet([1n, 0]));
      mirror.flush();
      await waiting;
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("failWaiters rejects all of them", async () => {
    const mirror = withWaiters();
    const a = mirror.whenObserved(1n, 0);
    const b = mirror.whenObserved(2n, 0);
    const failure = new Error("transport lost");
    mirror.failWaiters(failure);
    await expect(a).rejects.toBe(failure);
    await expect(b).rejects.toBe(failure);
  });
});
