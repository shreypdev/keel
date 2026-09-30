import { describe, expect, it, vi } from "vitest";
import { Signal, batch, setSignalErrorHandler, trackReads } from "../src/signal.js";

describe("Signal", () => {
  it("holds the initial value and reads it with get and peek", () => {
    const s = new Signal(3);
    expect(s.get()).toBe(3);
    expect(s.peek()).toBe(3);
  });

  it("notifies subscribers with the new value, not the current one", () => {
    const s = new Signal("a");
    const seen: string[] = [];
    s.subscribe((v) => seen.push(v));
    expect(seen).toEqual([]);
    s._set("b");
    s._set("c");
    expect(seen).toEqual(["b", "c"]);
    expect(s.peek()).toBe("c");
  });

  it("ignores a value equal to the current one (Object.is)", () => {
    const s = new Signal(NaN);
    const fn = vi.fn();
    s.subscribe(fn);
    s._set(NaN);
    expect(fn).not.toHaveBeenCalled();
    const list = [1];
    const l = new Signal(list);
    l.subscribe(fn);
    l._set(list);
    expect(fn).not.toHaveBeenCalled();
    l._set([1]);
    expect(fn).toHaveBeenCalledOnce();
  });

  it("unsubscribes, and unsubscribing twice is harmless", () => {
    const s = new Signal(0);
    const fn = vi.fn();
    const stop = s.subscribe(fn);
    expect(s.subscriberCount).toBe(1);
    stop();
    stop();
    expect(s.subscriberCount).toBe(0);
    s._set(1);
    expect(fn).not.toHaveBeenCalled();
  });

  it("allows the same function to subscribe twice, each with its own subscription", () => {
    const s = new Signal(0);
    const fn = vi.fn();
    const stopA = s.subscribe(fn);
    s.subscribe(fn);
    s._set(1);
    expect(fn).toHaveBeenCalledTimes(2);
    stopA();
    s._set(2);
    expect(fn).toHaveBeenCalledTimes(3);
  });

  it("lets a subscriber unsubscribe itself or others while being notified", () => {
    const s = new Signal(0);
    const order: string[] = [];
    let stopB: () => void = () => {};
    s.subscribe(() => {
      order.push("a");
      stopB();
    });
    stopB = s.subscribe(() => order.push("b"));
    s.subscribe(() => order.push("c"));
    s._set(1);
    expect(order).toEqual(["a", "c"]);
  });

  it("does not call subscribers added during a notification for that change", () => {
    const s = new Signal(0);
    const late = vi.fn();
    s.subscribe(() => {
      s.subscribe(late);
    });
    s._set(1);
    expect(late).not.toHaveBeenCalled();
    s._set(2);
    expect(late).toHaveBeenCalledWith(2);
  });

  it("isolates a throwing subscriber and reports it to the error handler", () => {
    const errors: unknown[] = [];
    const previous = setSignalErrorHandler((e) => errors.push(e));
    try {
      const s = new Signal(0);
      const after = vi.fn();
      const boom = new Error("subscriber failed");
      s.subscribe(() => {
        throw boom;
      });
      s.subscribe(after);
      s._set(1);
      expect(after).toHaveBeenCalledWith(1);
      expect(errors).toEqual([boom]);
    } finally {
      setSignalErrorHandler(previous);
    }
  });

  it("rethrows a subscriber's error from a microtask by default", async () => {
    const seen: unknown[] = [];
    const listener = (e: unknown) => seen.push(e);
    process.on("uncaughtException", listener);
    const originalListeners = process.listeners("uncaughtException").filter((l) => l !== listener);
    for (const l of originalListeners) process.off("uncaughtException", l);
    try {
      const s = new Signal(0);
      s.subscribe(() => {
        throw new Error("default handler");
      });
      s._set(1);
      await new Promise((resolve) => setTimeout(resolve, 5));
      expect(seen).toHaveLength(1);
      expect((seen[0] as Error).message).toBe("default handler");
    } finally {
      process.off("uncaughtException", listener);
      for (const l of originalListeners) process.on("uncaughtException", l as (e: Error) => void);
    }
  });

  it("is assignable to Signal<unknown> like generated stores need", () => {
    const signals: Signal<unknown>[] = [new Signal(1), new Signal("x"), new Signal<number[]>([])];
    expect(signals.map((s) => s.peek())).toEqual([1, "x", []]);
  });
});

describe("batch", () => {
  it("notifies once per signal, with the final value", () => {
    const a = new Signal(0);
    const b = new Signal(0);
    const seenA: number[] = [];
    const seenB: number[] = [];
    a.subscribe((v) => seenA.push(v));
    b.subscribe((v) => seenB.push(v));
    batch(() => {
      a._set(1);
      b._set(1);
      a._set(2);
      a._set(3);
      // Values are visible immediately; only the announcement waits.
      expect(a.peek()).toBe(3);
      expect(seenA).toEqual([]);
    });
    expect(seenA).toEqual([3]);
    expect(seenB).toEqual([1]);
  });

  it("nests: only the outermost batch announces", () => {
    const a = new Signal(0);
    const fn = vi.fn();
    a.subscribe(fn);
    batch(() => {
      batch(() => {
        a._set(1);
      });
      expect(fn).not.toHaveBeenCalled();
      a._set(2);
    });
    expect(fn).toHaveBeenCalledOnce();
    expect(fn).toHaveBeenCalledWith(2);
  });

  it("returns the callback's result and announces even when it throws", () => {
    const a = new Signal(0);
    const fn = vi.fn();
    a.subscribe(fn);
    expect(batch(() => 42)).toBe(42);
    expect(() =>
      batch(() => {
        a._set(1);
        throw new Error("nope");
      }),
    ).toThrow("nope");
    expect(fn).toHaveBeenCalledWith(1);
  });

  it("a subscriber that sets another signal is announced in the same drain", () => {
    const a = new Signal(0);
    const b = new Signal(0);
    const seen: string[] = [];
    a.subscribe((v) => {
      seen.push(`a=${v}`);
      b._set(v * 10);
    });
    b.subscribe((v) => seen.push(`b=${v}`));
    batch(() => {
      a._set(1);
    });
    expect(seen).toEqual(["a=1", "b=10"]);
  });

  it("does not queue signals nobody listens to", () => {
    const a = new Signal(0);
    batch(() => {
      a._set(1);
    });
    const fn = vi.fn();
    a.subscribe(fn);
    batch(() => {
      a._set(1);
    });
    expect(fn).not.toHaveBeenCalled();
  });

  it("a signal changed twice back to its original value inside a batch is still announced once", () => {
    const a = new Signal(1);
    const fn = vi.fn();
    a.subscribe(fn);
    batch(() => {
      a._set(2);
      a._set(1);
    });
    expect(fn).toHaveBeenCalledOnce();
    expect(fn).toHaveBeenCalledWith(1);
  });
});

describe("trackReads", () => {
  it("collects the signals read through get(), not through peek()", () => {
    const a = new Signal(1);
    const b = new Signal(2);
    const c = new Signal(3);
    const { value, signals } = trackReads(() => a.get() + b.get() + c.peek());
    expect(value).toBe(6);
    expect([...signals]).toEqual([a, b]);
  });

  it("nests and restores the outer collector", () => {
    const a = new Signal(1);
    const b = new Signal(2);
    const outer = trackReads(() => {
      a.get();
      const inner = trackReads(() => b.get());
      expect([...inner.signals]).toEqual([b]);
      return 0;
    });
    expect([...outer.signals]).toEqual([a]);
    // Outside any tracking a read has no effect.
    expect(a.get()).toBe(1);
  });

  it("restores the collector when the callback throws", () => {
    const a = new Signal(1);
    expect(() =>
      trackReads(() => {
        throw new Error("x");
      }),
    ).toThrow("x");
    const { signals } = trackReads(() => a.get());
    expect([...signals]).toEqual([a]);
  });
});
