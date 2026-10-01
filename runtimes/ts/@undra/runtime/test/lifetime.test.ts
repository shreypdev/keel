import { describe, expect, it, vi } from "vitest";
import type { UndraObject } from "../src/object.js";
import { isUndraClass, openUndra, rethrowLater } from "../src/lifetime.js";
import { deferred, microtasks } from "./support/harness.js";

/** The part of an `UndraObject` that `openUndra` touches. */
function fakeObject(): UndraObject & { closes: number } {
  const object = {
    closes: 0,
    close() {
      object.closes++;
    },
  };
  return object as unknown as UndraObject & { closes: number };
}

describe("openUndra", () => {
  it("hands the object to onReady and closes it when ended", async () => {
    const object = fakeObject();
    const onReady = vi.fn();
    const end = openUndra(async () => object, onReady, vi.fn());
    await microtasks();
    expect(onReady).toHaveBeenCalledWith(object);
    expect(object.closes).toBe(0);
    end();
    end();
    expect(object.closes).toBe(1);
  });

  it("closes an object that arrives after it was ended, and reports nothing", async () => {
    const object = fakeObject();
    const gate = deferred<UndraObject>();
    const onReady = vi.fn();
    const onError = vi.fn();
    const end = openUndra(() => gate.promise, onReady, onError);
    end();
    gate.resolve(object);
    await microtasks();
    expect(object.closes).toBe(1);
    expect(onReady).not.toHaveBeenCalled();
    expect(onError).not.toHaveBeenCalled();
  });

  it("reports a creation that fails, unless it was ended", async () => {
    const failure = new Error("no");
    const onError = vi.fn();
    openUndra(() => Promise.reject(failure), vi.fn(), onError);
    await microtasks();
    expect(onError).toHaveBeenCalledWith(failure);

    const late = deferred<UndraObject>();
    const lateError = vi.fn();
    const end = openUndra(() => late.promise, vi.fn(), lateError);
    end();
    late.reject(failure);
    await microtasks();
    expect(lateError).not.toHaveBeenCalled();
  });

  it("treats a factory that throws before it returns a promise as a failed creation", async () => {
    const failure = new Error("sync");
    const onError = vi.fn();
    openUndra(
      () => {
        throw failure;
      },
      vi.fn(),
      onError,
    );
    await microtasks();
    expect(onError).toHaveBeenCalledWith(failure);
  });
});

describe("isUndraClass", () => {
  it("tells a generated class (a static create) from a factory", () => {
    class Generated {
      static async create(): Promise<UndraObject> {
        return fakeObject();
      }
    }
    expect(isUndraClass(Generated)).toBe(true);
    expect(isUndraClass({ create: async () => fakeObject() })).toBe(true);
    expect(isUndraClass(async () => fakeObject())).toBe(false);
  });
});

describe("rethrowLater", () => {
  it("throws from a microtask, not at the call", () => {
    const queued: Array<() => void> = [];
    const spy = vi.spyOn(globalThis, "queueMicrotask").mockImplementation((fn) => {
      queued.push(fn);
    });
    try {
      const error = new Error("late");
      expect(() => rethrowLater(error)).not.toThrow();
      expect(queued).toHaveLength(1);
      expect(() => queued[0]?.()).toThrow(error);
    } finally {
      spy.mockRestore();
    }
  });
});
