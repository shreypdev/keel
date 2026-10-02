import { createEffect, createMemo, createRoot } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import { UndraCore } from "../src/core.js";
import type { UndraClass } from "../src/lifetime.js";
import { Signal } from "../src/signal.js";
import { useUndra, useSignal } from "../src/solid.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { deferred, macrotask, microtasks, track } from "./support/harness.js";
import { CounterStore, str, u32, vecU32 } from "./support/store.js";

/** The Solid adapter against Solid's own reactivity (its browser build, which needs no DOM). */

describe("useSignal (solid)", () => {
  it("is an accessor with the current value that follows the signal", () => {
    const count = new Signal(3);
    createRoot((dispose) => {
      const value = useSignal(count);
      expect(value()).toBe(3);
      count._set(4);
      expect(value()).toBe(4);
      dispose();
    });
  });

  it("is tracked: effects and memos that read it run again for each change", () => {
    const count = new Signal(0);
    const seen: number[] = [];
    const doubled = createRoot((dispose) => {
      const value = useSignal(count);
      createEffect(() => {
        seen.push(value());
      });
      const memo = createMemo(() => value() * 2);
      return { memo, dispose };
    });
    count._set(1);
    count._set(2);
    expect(seen).toEqual([0, 1, 2]);
    expect(doubled.memo()).toBe(4);
    doubled.dispose();
  });

  it("ends the subscription with the owner", () => {
    const count = new Signal(0);
    createRoot((dispose) => {
      useSignal(count);
      expect(count.subscriberCount).toBe(1);
      dispose();
    });
    expect(count.subscriberCount).toBe(0);
  });

  it("works without an owner, and then lives as long as the signal", () => {
    const count = new Signal(1);
    const value = useSignal(count);
    count._set(2);
    expect(value()).toBe(2);
  });

  it("holds a value that is itself a function, which Solid would otherwise call", () => {
    const fn = (): number => 1;
    const holder = new Signal<() => number>(fn);
    createRoot((dispose) => {
      const value = useSignal(holder);
      expect(value()).toBe(fn);
      const next = (): number => 2;
      holder._set(next);
      expect(value()).toBe(next);
      dispose();
    });
  });
});

async function setup() {
  const fake = new FakeCoreTransport({ synchronous: true });
  const core = track(
    await UndraCore.attach(fake, { expectedSchemaHash: SCHEMA, shared: false, adapters: { log: { log() {} }, http: null, timer: null } }),
  );
  let next = 0x1_0000_0001n;
  const created: CounterStore[] = [];
  const Counter: UndraClass<CounterStore> = {
    create: async (target = core) => {
      const handle = next++;
      fake.store(
        handle,
        new Map([
          [0, u32(1)],
          [1, str("one")],
          [2, vecU32([1, 2, 3])],
        ]),
      );
      const store = await CounterStore.create(target, handle);
      created.push(store);
      return store;
    },
  };
  return { fake, core, Counter, created };
}

describe("useUndra (solid)", () => {
  it("is undefined until the store exists, then holds it", async () => {
    const { Counter } = await setup();
    await createRoot(async (dispose) => {
      const store = useUndra(Counter);
      expect(store()).toBeUndefined();
      await microtasks(10);
      expect(store()).toBeInstanceOf(CounterStore);
      dispose();
    });
  });

  it("lets useSignal follow the store it creates", async () => {
    const { Counter, fake, created } = await setup();
    await createRoot(async (dispose) => {
      const store = useUndra(Counter);
      await microtasks(10);
      const count = useSignal((store() as CounterStore).count);
      expect(count()).toBe(1);
      fake.setSignal(created[0]?.handle as bigint, 0, u32(8));
      await fake.settle();
      expect(count()).toBe(8);
      dispose();
    });
  });

  it("closes the store when the owner ends", async () => {
    const { Counter, created, fake } = await setup();
    await createRoot(async (dispose) => {
      useUndra(Counter);
      await microtasks(10);
      const store = created[0] as CounterStore;
      expect(store.closed).toBe(false);
      dispose();
      expect(store.closed).toBe(true);
      expect(fake.released).toEqual([store.handle]);
    });
  });

  it("closes a store that arrives after the owner ended", async () => {
    const { Counter, created } = await setup();
    const gate = deferred();
    const slow: UndraClass<CounterStore> = {
      create: async (core) => {
        await gate.promise;
        return Counter.create(core);
      },
    };
    const store = createRoot((dispose) => {
      const accessor = useUndra(slow);
      dispose();
      return accessor;
    });
    gate.resolve();
    await microtasks(20);
    await macrotask(); // the close waits one turn (see `openUndra`)
    await macrotask();
    expect(created).toHaveLength(1);
    expect(created[0]?.closed).toBe(true);
    expect(store()).toBeUndefined();
  });

  it("creates in the core it is given", async () => {
    const { core } = await setup();
    const create = vi.fn(async () => {
      throw new Error("not needed");
    });
    const queued: Array<() => void> = [];
    const spy = vi.spyOn(globalThis, "queueMicrotask").mockImplementation((fn) => {
      queued.push(fn);
    });
    try {
      createRoot((dispose) => {
        useUndra({ create }, core);
        dispose();
      });
      expect(create).toHaveBeenCalledWith(core);
      await microtasks();
    } finally {
      spy.mockRestore();
    }
  });
});
