import { type Ref, computed, effectScope, ref, shallowRef, watch } from "vue";
import { describe, expect, it, vi } from "vitest";
import { UndraCore } from "../src/core.js";
import type { UndraClass } from "../src/lifetime.js";
import { Signal } from "../src/signal.js";
import { useUndra, useSignal } from "../src/vue.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { deferred, microtasks, track } from "./support/harness.js";
import { CounterStore, str, u32, vecU32 } from "./support/store.js";

/** The Vue adapter against Vue's own reactivity (it needs no DOM). */

describe("useSignal (vue)", () => {
  it("holds the current value and follows the signal at once", () => {
    const count = new Signal(3);
    const scope = effectScope();
    const value = scope.run(() => useSignal(count));
    expect(value?.value).toBe(3);
    count._set(4);
    expect(value?.value).toBe(4);
    scope.stop();
  });

  it("is a shallow ref: the value is the list itself, not a proxy of it", () => {
    const items = [{ id: 1 }, { id: 2 }];
    const list = new Signal(items);
    const scope = effectScope();
    const value = scope.run(() => useSignal(list));
    expect(value?.value).toBe(items);
    scope.stop();
  });

  it("is reactive: a watcher and a computed of the ref follow each change", () => {
    const count = new Signal(0);
    const scope = effectScope();
    const seen: number[] = [];
    const doubled = scope.run(() => {
      const value = useSignal(count);
      watch(value, (next) => seen.push(next), { flush: "sync" });
      return computed(() => value.value * 2);
    });
    count._set(1);
    count._set(2);
    expect(seen).toEqual([1, 2]);
    expect(doubled?.value).toBe(4);
    scope.stop();
  });

  it("ends the subscription with the scope", () => {
    const count = new Signal(0);
    const scope = effectScope();
    scope.run(() => useSignal(count));
    expect(count.subscriberCount).toBe(1);
    scope.stop();
    expect(count.subscriberCount).toBe(0);
  });

  it("follows a getter: undefined while there is no signal, then the signal, then another", () => {
    const a = new Signal(1);
    const b = new Signal(2);
    const source = shallowRef<Signal<number> | undefined>(undefined);
    const scope = effectScope();
    const value = scope.run(() => useSignal(() => source.value));
    expect(value?.value).toBeUndefined();
    source.value = a;
    expect(value?.value).toBe(1);
    a._set(10);
    expect(value?.value).toBe(10);
    source.value = b;
    expect(value?.value).toBe(2);
    expect([a.subscriberCount, b.subscriberCount]).toEqual([0, 1]);
    a._set(11);
    expect(value?.value).toBe(2);
    source.value = undefined;
    expect(value?.value).toBeUndefined();
    expect(b.subscriberCount).toBe(0);
    scope.stop();
  });

  it("follows a ref of a signal, also a deep one that wraps it in a proxy", () => {
    const count = new Signal(5);
    const scope = effectScope();
    // `ref(signal)` is a deep ref: its value is a reactive proxy of the signal (TypeScript already
    // objects to it, hence the cast), and a proxy cannot reach the signal's private fields.
    const deep = ref(count) as unknown as Ref<Signal<number>>;
    const value = scope.run(() => useSignal(deep));
    expect(value?.value).toBe(5);
    count._set(6);
    expect(value?.value).toBe(6);
    scope.stop();
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

describe("useUndra (vue)", () => {
  it("is undefined until the store exists, then holds it", async () => {
    const { Counter } = await setup();
    const scope = effectScope();
    const store = scope.run(() => useUndra(Counter));
    expect(store?.value).toBeUndefined();
    await microtasks(10);
    expect(store?.value).toBeInstanceOf(CounterStore);
    scope.stop();
  });

  it("lets useSignal follow the store it creates", async () => {
    const { fake, Counter, created } = await setup();
    const scope = effectScope();
    const count = scope.run(() => {
      const store = useUndra(Counter);
      return useSignal(() => store.value?.count);
    });
    expect(count?.value).toBeUndefined();
    await microtasks(10);
    expect(count?.value).toBe(1);
    fake.setSignal(created[0]?.handle as bigint, 0, u32(8));
    await fake.settle();
    expect(count?.value).toBe(8);
    scope.stop();
  });

  it("closes the store when the scope ends", async () => {
    const { Counter, created, fake } = await setup();
    const scope = effectScope();
    scope.run(() => useUndra(Counter));
    await microtasks(10);
    const store = created[0] as CounterStore;
    expect(store.closed).toBe(false);
    scope.stop();
    expect(store.closed).toBe(true);
    expect(fake.released).toEqual([store.handle]);
  });

  it("closes a store that arrives after the scope ended", async () => {
    const { Counter, created } = await setup();
    const gate = deferred();
    const slow: UndraClass<CounterStore> = {
      create: async (core) => {
        await gate.promise;
        return Counter.create(core);
      },
    };
    const scope = effectScope();
    const store = scope.run(() => useUndra(slow));
    scope.stop();
    gate.resolve();
    await microtasks(20);
    expect(created).toHaveLength(1);
    expect(created[0]?.closed).toBe(true);
    expect(store?.value).toBeUndefined();
  });

  it("creates in the core it is given", async () => {
    const { core } = await setup();
    const create = vi.fn(async () => {
      throw new Error("not needed");
    });
    const scope = effectScope();
    scope.run(() => useUndra({ create }, core));
    expect(create).toHaveBeenCalledWith(core);
    scope.stop();
    await microtasks();
  });

  it("rethrows a failed creation from a microtask", async () => {
    const queued: Array<() => void> = [];
    const spy = vi.spyOn(globalThis, "queueMicrotask").mockImplementation((fn) => {
      queued.push(fn);
    });
    try {
      const failure = new Error("the core said no");
      const scope = effectScope();
      scope.run(() => useUndra({ create: () => Promise.reject(failure) }));
      await microtasks(10);
      expect(queued).toHaveLength(1);
      expect(() => queued[0]?.()).toThrow(failure);
      scope.stop();
    } finally {
      spy.mockRestore();
    }
  });
});
