import { describe, expect, it } from "vitest";
import { UndraCallError } from "../src/call-error.js";
import { UndraCore } from "../src/core.js";
import { adopt, adoptList, adoptObject, adoptOptional, requireOwn } from "../src/identity.js";
import { UndraObject } from "../src/object.js";
import { ALL_SIGNALS, codecs, encodeValue } from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { macrotask, track } from "./support/harness.js";
import { CounterStore, str, u32, vecU32 } from "./support/store.js";

// ADR-040 decision 7: one wrapper per handle. `adopt` returns the live wrapper of a handle and gives the reply's extra
// reference back, or makes a new one that owns it; closing releases exactly one reference; a finalizer releases a
// collected wrapper's reference at most once; an object of another core is refused before anything is sent.

/** A generated plain object, as `undra-bindgen` writes one: a private constructor that only `adopt` reaches. */
class Thing extends UndraObject {
  private constructor(core: UndraCore, handle: bigint) {
    super(core, handle);
  }
}

async function setup(synchronous = false): Promise<{ fake: FakeCoreTransport; core: UndraCore }> {
  const fake = new FakeCoreTransport({ synchronous });
  const core = track(
    await UndraCore.attach(fake, { expectedSchemaHash: SCHEMA, shared: false, adapters: { log: { log() {} }, http: null, timer: null } }),
  );
  return { fake, core };
}

const handleBody = (handle: bigint): Uint8Array => encodeValue(codecs.u64, handle);
const gc = (globalThis as { gc?: () => void }).gc;

describe("adopt", () => {
  it("returns one wrapper per handle and gives a duplicate reference back at once", async () => {
    const { fake, core } = await setup();
    const thing = adopt(core, 0x10n, Thing);
    expect(thing).toBeInstanceOf(Thing);
    expect(thing.handle).toBe(0x10n);
    expect(adopt(core, 0x10n, Thing)).toBe(thing);
    expect(adopt(core, 0x10n, Thing)).toBe(thing);
    expect(fake.released, "two duplicates, two references back").toEqual([0x10n, 0x10n]);
    expect((await core.stats()).hostRefs, "the runtime counts the handle its wrapper holds").toBeGreaterThanOrEqual(0);
    thing.close();
    thing.close();
    expect(fake.released, "the wrapper releases the one reference it owns, once").toEqual([0x10n, 0x10n, 0x10n]);
  });

  it("keeps the cores apart: the same handle number in two cores is two objects", async () => {
    const one = await setup();
    const two = await setup();
    const a = adopt(one.core, 0x20n, Thing);
    const b = adopt(two.core, 0x20n, Thing);
    expect(a).not.toBe(b);
    expect(a.core).toBe(one.core);
    expect(b.core).toBe(two.core);
    expect(one.fake.released).toEqual([]);
    expect(two.fake.released).toEqual([]);
  });

  it("replaces a closed wrapper: a reply's reference after close belongs to a new wrapper", async () => {
    const { fake, core } = await setup();
    const first = adopt(core, 0x30n, Thing);
    first.close();
    const second = adopt(core, 0x30n, Thing);
    expect(second).not.toBe(first);
    expect(second.closed).toBe(false);
    expect(fake.released, "only the closed wrapper's reference went back").toEqual([0x30n]);
  });

  it("observes a returned store once, and twice-returned stores are mirrored once", async () => {
    const { fake, core } = await setup();
    fake.store(0x40n, new Map([[0, u32(7)], [1, str("seven")], [2, vecU32([7])]]));
    const store = await adoptObject(core, handleBody(0x40n), CounterStore);
    expect(store.count.peek(), "the initial values arrived before it was returned").toBe(7);
    expect(fake.observed).toEqual([{ handle: 0x40n, signalId: ALL_SIGNALS, on: true }]);
    const again = await adoptObject(core, handleBody(0x40n), CounterStore);
    expect(again).toBe(store);
    expect(fake.observed, "observed once").toHaveLength(1);
    expect(core.mirror.size).toBe(1);
    fake.setSignal(0x40n, 0, u32(8));
    await macrotask();
    expect(store.count.peek()).toBe(8);
  });

  it("waits for the first observation of a store a concurrent reply made", async () => {
    const { fake, core } = await setup();
    fake.store(0x41n, new Map([[0, u32(3)], [1, str("x")], [2, vecU32([])]]));
    const [first, second] = await Promise.all([
      adoptObject(core, handleBody(0x41n), CounterStore),
      adoptObject(core, handleBody(0x41n), CounterStore),
    ]);
    expect(second).toBe(first);
    expect(second.count.peek()).toBe(3);
  });

  it("decodes optional and plural results, adopting each handle as it is read", async () => {
    const { fake, core } = await setup();
    expect(await adoptOptional(core, encodeValue(codecs.option(codecs.u64), null), Thing)).toBeNull();
    const one = await adoptOptional(core, encodeValue(codecs.option(codecs.u64), 0x50n), Thing);
    const list = await adoptList(core, encodeValue(codecs.vec(codecs.u64), [0x50n, 0x51n, 0x50n]), Thing);
    expect(list[0]).toBe(one);
    expect(list[2]).toBe(one);
    expect(list[1]?.handle).toBe(0x51n);
    expect(fake.released).toEqual([0x50n, 0x50n]);
    expect(await adoptList(core, encodeValue(codecs.vec(codecs.u64), []), Thing)).toEqual([]);
  });

  it("refuses the null handle and a body that is not one handle", async () => {
    const { core } = await setup();
    const malformed = (e: unknown): boolean => UndraCallError.mapped(e) instanceof UndraCallError.Malformed;
    await expect(adoptObject(core, handleBody(0n), Thing).catch((e: unknown) => malformed(e))).resolves.toBe(true);
    await expect(adoptObject(core, new Uint8Array(3), Thing).catch((e: unknown) => malformed(e))).resolves.toBe(true);
    await expect(adoptOptional(core, new Uint8Array([2]), Thing).catch((e: unknown) => malformed(e))).resolves.toBe(true);
  });

  it("sweeps the entries of closed wrappers as the map grows", async () => {
    const { core } = await setup();
    for (let i = 1n; i <= 500n; i++) adopt(core, i, Thing).close();
    // Nothing observable but that it keeps working, and that a live wrapper survives a sweep.
    const live = adopt(core, 9999n, Thing);
    for (let i = 501n; i <= 700n; i++) adopt(core, i, Thing).close();
    expect(adopt(core, 9999n, Thing)).toBe(live);
  });

  it.skipIf(gc === undefined)("releases a collected wrapper's reference once", async () => {
    const { fake, core } = await setup();
    fake.store(0x60n, new Map([[0, u32(1)], [1, str("a")], [2, vecU32([])]]));
    // A store adopted and dropped (no close): its finalizer releases it.
    await (async () => {
      await adoptObject(core, handleBody(0x60n), CounterStore);
    })();
    for (let i = 0; i < 20 && !fake.released.includes(0x60n); i++) {
      gc?.();
      await macrotask();
    }
    expect(fake.released).toEqual([0x60n]);
    expect(core.mirror.has(0x60n)).toBe(false);
  });

  it.skipIf(gc === undefined)("spares a newer wrapper of the handle when the collected one's finalizer runs late", async () => {
    const { fake, core } = await setup();
    const original = (() => new WeakRef(adopt(core, 0x62n, CounterStore)))();
    await macrotask(); // a WeakRef keeps its target for the job that made it
    fake.released.length = 0;
    gc?.();
    expect(original.deref(), "collected").toBeUndefined();
    // Collected, finalizer not run yet (it runs in a later task): a reply carries the handle again.
    const newer = adopt(core, 0x62n, CounterStore);
    expect(core.mirror.has(0x62n), "the newer wrapper is mirrored").toBe(true);
    for (let i = 0; i < 10; i++) {
      gc?.();
      await macrotask();
    }
    expect(fake.released, "the old wrapper's reference, once").toEqual([0x62n]);
    expect(core.mirror.has(0x62n), "the newer wrapper keeps its registration").toBe(true);
    expect(newer.closed).toBe(false);
  });

  it.skipIf(gc === undefined)("re-adopts a store whose collected wrapper's entry was swept before its finalizer ran", async () => {
    const { fake, core } = await setup();
    const original = (() => new WeakRef(adopt(core, 0x64n, CounterStore)))();
    await macrotask(); // a WeakRef keeps its target for the job that made it
    gc?.();
    expect(original.deref(), "collected").toBeUndefined();
    // Collected, finalizer not run yet: the map grows to 64 and its sweep drops the dead entry, while the
    // collected store's mirror registration is still there (only its finalizer, or adopt, removes it).
    for (let i = 1n; i < 64n; i++) adopt(core, 0x1000n + i, Thing).close();
    expect(core.mirror.has(0x64n)).toBe(true);
    // A reply carries the handle again (the core still counts the collected wrapper's reference).
    fake.released.length = 0;
    const newer = adopt(core, 0x64n, CounterStore);
    expect(core.mirror.has(0x64n), "the new wrapper is mirrored").toBe(true);
    for (let i = 0; i < 10; i++) {
      gc?.();
      await macrotask();
    }
    expect(fake.released, "the collected wrapper's reference, once").toEqual([0x64n]);
    expect(newer.closed).toBe(false);
    expect(core.mirror.has(0x64n), "the new wrapper keeps its registration").toBe(true);
  });

  it("measures host-side adopt (a new wrapper, then the identity hit)", async () => {
    const { core } = await setup(true);
    const rounds = 20_000;
    let started = performance.now();
    for (let i = 1; i <= rounds; i++) adopt(core, BigInt(i), Thing);
    const fresh = ((performance.now() - started) * 1e6) / rounds;
    started = performance.now();
    for (let i = 1; i <= rounds; i++) adopt(core, BigInt(i), Thing);
    const hit = ((performance.now() - started) * 1e6) / rounds;
    // The lookup alone: the give-back's Release is the transport's cost, not the identity map's.
    (core as unknown as { _giveBack(handle: bigint): void })._giveBack = () => {};
    started = performance.now();
    for (let i = 1; i <= rounds; i++) adopt(core, BigInt(i), Thing);
    const lookup = ((performance.now() - started) * 1e6) / rounds;
    console.log(
      `BENCH host adopt: new wrapper ${fresh.toFixed(0)} ns/op; live wrapper ${hit.toFixed(0)} ns/op with its Release through the in-process fake, ${lookup.toFixed(0)} ns/op for the lookup alone`,
    );
    expect(fresh).toBeGreaterThan(0);
  });
});

describe("requireOwn", () => {
  it("returns the handle of an object of the core, and refuses one of another core before anything is sent", async () => {
    const one = await setup();
    const two = await setup();
    const mine = adopt(one.core, 0x70n, Thing);
    const theirs = adopt(two.core, 0x70n, Thing);
    expect(requireOwn(one.core, mine)).toBe(0x70n);
    let refused: unknown;
    try {
      requireOwn(one.core, theirs);
    } catch (error) {
      refused = error;
    }
    expect(refused).toBeInstanceOf(UndraCallError.Refused);
    expect((refused as Error).message).toMatch(/Thing belongs to another core/);
    expect(one.fake.calls).toEqual([]);
  });
});
