import { describe, expect, it, vi } from "vitest";
import { KeelCore } from "../src/core.js";
import { KeelObject } from "../src/object.js";
import { ALL_SIGNALS, ChangeOp, KeelWriter, encodePatch, codecs, type PatchOp } from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { macrotask, track } from "./support/harness.js";
import { CounterStore, str, u32, vecU32 } from "./support/store.js";

const H = 0x0000_0001_0000_0004n;

async function setup(options: { synchronous?: boolean; observeTimeoutMs?: number; onError?: (e: unknown) => void } = {}) {
  const fake = new FakeCoreTransport({ synchronous: options.synchronous ?? false });
  const core = track(
    await KeelCore.attach(fake, {
      expectedSchemaHash: SCHEMA,
      shared: false,
      adapters: { log: { log() {} }, http: null, timer: null },
      ...(options.observeTimeoutMs !== undefined && { observeTimeoutMs: options.observeTimeoutMs }),
      ...(options.onError && { onError: options.onError }),
    }),
  );
  fake.store(
    H,
    new Map([
      [0, u32(1)],
      [1, str("one")],
      [2, vecU32([1, 2, 3])],
    ]),
  );
  return { fake, core };
}

describe("observe", () => {
  it("resolves once the initial values have been applied (worker and remote modes)", async () => {
    const { fake, core } = await setup();
    const store = await CounterStore.create(core, H);
    expect(store.count.peek()).toBe(1);
    expect(store.label.peek()).toBe("one");
    expect(store.items.peek()).toEqual([1, 2, 3]);
    expect(fake.observed).toEqual([{ handle: H, signalId: ALL_SIGNALS, on: true }]);
  });

  it("does not resolve before the change-set arrives", async () => {
    const { core } = await setup();
    const store = CounterStore.unobserved(core, 0x99n); // the fake has no store 0x99
    let resolved = false;
    void core.observe(0x99n, ALL_SIGNALS, true).then(() => {
      resolved = true;
    });
    await macrotask();
    await macrotask();
    expect(resolved).toBe(false);
    store.close();
  });

  it("is applied in the same task as the change-set, before the first render could read it", async () => {
    const { core } = await setup();
    const store = CounterStore.unobserved(core, H);
    const seen: number[] = [];
    store.count.subscribe((v) => seen.push(v));
    await core.observe(H, ALL_SIGNALS, true);
    expect(seen).toEqual([1]);
  });

  it("applies the initial change-set before observe returns for an in-process core", async () => {
    const { fake, core } = await setup({ synchronous: true });
    const store = CounterStore.unobserved(core, H);
    const promise = core.observe(H, ALL_SIGNALS, true);
    // Synchronously visible, no microtask needed.
    expect(store.count.peek()).toBe(1);
    expect(store.items.peek()).toEqual([1, 2, 3]);
    await promise;
    expect(fake.observed).toHaveLength(1);
  });

  it("observing a single signal resolves on that signal's entry", async () => {
    const { core } = await setup();
    const store = CounterStore.unobserved(core, H);
    await core.observe(H, 1, true);
    expect(store.label.peek()).toBe("one");
    expect(store.count.peek()).toBe(0);
  });

  it("stopping resolves at once and sends Observe off", async () => {
    const { fake, core } = await setup();
    await core.observe(H, ALL_SIGNALS, false);
    expect(fake.observed).toEqual([{ handle: H, signalId: ALL_SIGNALS, on: false }]);
  });

  it("rejects when nothing arrives within observeTimeoutMs", async () => {
    const { core } = await setup({ observeTimeoutMs: 20 });
    await expect(core.observe(0x77n, ALL_SIGNALS, true)).rejects.toMatchObject({ kind: "observe" });
  });

  it("releasing the store settles a pending observe", async () => {
    const { core } = await setup({ observeTimeoutMs: 0 });
    CounterStore.unobserved(core, 0x55n);
    const pending = core.observe(0x55n, ALL_SIGNALS, true);
    core.release(0x55n);
    await expect(pending).resolves.toBeUndefined();
  });

  it("rejects when the transport refuses the message", async () => {
    const { fake, core } = await setup();
    vi.spyOn(fake, "send").mockImplementation(() => {
      throw new Error("send failed");
    });
    await expect(core.observe(H, 0, true)).rejects.toThrow("send failed");
  });
});

describe("mirror through a core", () => {
  it("coalesces change-sets that arrive in one macrotask into one flush and one notification", async () => {
    const { fake, core } = await setup();
    const store = await CounterStore.create(core, H);
    const seen: number[] = [];
    store.count.subscribe((v) => seen.push(v));
    fake.burst(() => {
      fake.setSignal(H, 0, u32(10));
      fake.setSignal(H, 0, u32(11));
      fake.setSignal(H, 0, u32(12));
    });
    await fake.settle();
    expect(seen).toEqual([12]);
  });

  it("change-sets that arrive in separate macrotasks flush separately", async () => {
    const { fake, core } = await setup();
    const store = await CounterStore.create(core, H);
    const seen: number[] = [];
    store.count.subscribe((v) => seen.push(v));
    fake.setSignal(H, 0, u32(10));
    fake.setSignal(H, 0, u32(11));
    await fake.settle();
    expect(seen).toEqual([10, 11]);
  });

  it("a reply is delivered after the change-set that preceded it (read your writes)", async () => {
    const { fake, core } = await setup();
    const store = await CounterStore.create(core, H);
    fake.on(1, (_c, r) => {
      fake.setSignal(H, 0, u32(99));
      r.ok();
    });
    await core.call({ target: 0 }, 1, new Uint8Array(0));
    expect(store.count.peek()).toBe(99);
  });

  it("applies keyed patches to the current list", async () => {
    const { fake, core } = await setup();
    const store = await CounterStore.create(core, H);
    const ops: PatchOp<number>[] = [
      { op: "insert", index: 1, item: 9 },
      { op: "remove", index: 0 },
    ];
    const w = new KeelWriter();
    encodePatch(w, ops, codecs.u32);
    fake.emitChangeSet([{ handle: H, signalId: 2, op: ChangeOp.KeyedPatch, value: w.finish() }]);
    await fake.settle();
    expect(store.items.peek()).toEqual([9, 2, 3]);
  });

  it("reports a change-set that does not decode, keeps the stores as they were, and goes on", async () => {
    const errors: unknown[] = [];
    const { fake, core } = await setup({ onError: (e) => errors.push(e) });
    const store = await CounterStore.create(core, H);
    fake.emitRawChangeSet(new Uint8Array([1, 2, 3]));
    fake.setSignal(H, 0, u32(5));
    await fake.settle();
    expect(errors).toHaveLength(1);
    expect(store.count.peek()).toBe(5);
  });

  it("reports an out-of-bounds patch thrown from a store", async () => {
    const errors: unknown[] = [];
    const { fake, core } = await setup({ onError: (e) => errors.push(e) });
    const store = await CounterStore.create(core, H);
    const w = new KeelWriter();
    encodePatch(w, [{ op: "remove", index: 50 }], codecs.u32);
    fake.emitChangeSet([{ handle: H, signalId: 2, op: ChangeOp.KeyedPatch, value: w.finish() }]);
    await fake.settle();
    expect(errors).toHaveLength(1);
    expect(store.items.peek()).toEqual([1, 2, 3]);
  });
});

describe("KeelObject and KeelStore", () => {
  class Plain extends KeelObject {
    static make(core: KeelCore, handle: bigint): Plain {
      return new Plain(core, handle);
    }
  }

  it("exposes core and handle and releases the handle on close, once", async () => {
    const { fake, core } = await setup();
    const object = Plain.make(core, 12n);
    expect(object.core).toBe(core);
    expect(object.handle).toBe(12n);
    expect(object.closed).toBe(false);
    object.close();
    object.close();
    expect(object.closed).toBe(true);
    expect(fake.released).toEqual([12n]);
  });

  it("supports `using` through Symbol.dispose", async () => {
    const { fake, core } = await setup();
    {
      using object = Plain.make(core, 13n);
      expect(object.closed).toBe(false);
    }
    expect(fake.released).toEqual([13n]);
    const dispose = (Symbol as { dispose?: symbol }).dispose ?? Symbol.for("Symbol.dispose");
    expect(typeof (Plain.make(core, 14n) as unknown as Record<symbol, unknown>)[dispose]).toBe("function");
  });

  it("a store registers with the mirror on construction and unregisters on close", async () => {
    const { fake, core } = await setup();
    const store = CounterStore.unobserved(core, H);
    expect(core.mirror.has(H)).toBe(true);
    store.close();
    expect(core.mirror.has(H)).toBe(false);
    expect(fake.released).toEqual([H]);
    // Updates that were already on their way are dropped, not applied.
    fake.setSignal(H, 0, u32(77));
    await fake.settle();
    expect(store.count.peek()).toBe(0);
    expect(core.mirror.dropped).toBe(1);
  });

  it("a store lists its signals in signal-id order", async () => {
    const { core } = await setup();
    const store = CounterStore.unobserved(core, H);
    expect((store as unknown as { _signals: unknown[] })._signals).toEqual([store.count, store.label, store.items]);
  });

  it("two stores cannot mirror the same handle", async () => {
    const { core } = await setup();
    CounterStore.unobserved(core, H);
    expect(() => CounterStore.unobserved(core, H)).toThrow(/already registered/);
  });

  it("closing a store on a closed core is harmless", async () => {
    const { core } = await setup();
    const store = CounterStore.unobserved(core, H);
    core.close();
    expect(() => store.close()).not.toThrow();
  });

  it("the mirror holds stores weakly, so an abandoned store can be collected", async () => {
    const gc = (globalThis as { gc?: () => void }).gc;
    if (gc === undefined) return; // needs --expose-gc; the finalizer path is covered below
    const { core } = await setup();
    let store: CounterStore | null = CounterStore.unobserved(core, H);
    const ref = new WeakRef(store);
    store = null;
    // `WeakRef.deref` keeps its target alive until the end of the current job, so collect first, look afterwards.
    for (let i = 0; i < 20; i++) {
      await macrotask();
      gc();
      if (ref.deref() === undefined) break;
    }
    expect(ref.deref()).toBeUndefined();
    // The finalizer then releases the handle and the mirror registration.
    for (let i = 0; i < 20 && core.mirror.has(H); i++) {
      await macrotask();
      gc();
    }
    expect(core.mirror.has(H)).toBe(false);
  });
});
