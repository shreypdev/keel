import { describe, expect, it, vi } from "vitest";
import { Mirror } from "../src/mirror.js";
import { Signal } from "../src/signal.js";
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
      changeSet([1n, 0, bytes(1)], [2n, 5, bytes(2, 2), ChangeOp.KeyedPatch], [1n, 1, bytes(), ChangeOp.LazyInvalidated]),
    );
    expect(seen).toEqual([]); // nothing until the flush
    expect(mirror.pending).toBe(3);
    mirror.flush();
    expect(seen).toEqual([
      [0, ChangeOp.FullValue, [1]],
      ["two", 5, ChangeOp.KeyedPatch, [2, 2]],
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

describe("Mirror.whenObserved", () => {
  it("resolves after an entry for the signal has been applied and announced", async () => {
    const mirror = new Mirror();
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
    const mirror = new Mirror();
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
    const mirror = new Mirror();
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
      const mirror = new Mirror();
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
      const mirror = new Mirror();
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
    const mirror = new Mirror();
    const a = mirror.whenObserved(1n, 0);
    const b = mirror.whenObserved(2n, 0);
    const failure = new Error("transport lost");
    mirror.failWaiters(failure);
    await expect(a).rejects.toBe(failure);
    await expect(b).rejects.toBe(failure);
  });
});
