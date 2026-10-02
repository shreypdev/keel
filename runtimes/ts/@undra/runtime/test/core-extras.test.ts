import { afterEach, describe, expect, it, vi } from "vitest";
import type { AppState, LifecycleAdapter } from "../src/adapters/types.js";
import { SCHEMA, FakeCoreTransport } from "./support/fake-core.js";
import { captureLog, macrotask } from "./support/harness.js";

/*
 * `stats` and `runInBackground` load on their first call (ADR-057, D5: each already answered with a promise, so SPEC 17.1's
 * signatures are unchanged): `core-extras.ts` is a chunk of its own. `snapshot` and `restore` run at the call (they are ordered with the
 * calls around them, `snapshot.test.ts`): only the class of a refused restore is in the chunk. A page that never asks does not load
 * it, not even to drain its background work when it is hidden; a chunk that cannot be fetched is a typed failure (R6) and the next call
 * tries the import again.
 */

afterEach(() => {
  vi.doUnmock("../src/core-extras.js");
  vi.unstubAllGlobals();
  vi.resetModules();
});

/** The runtime's modules from a fresh registry (their classes are not the ones this file imported at the top), with `core-extras.js` counted. */
async function fresh(gate: (attempt: number) => void = () => {}) {
  vi.resetModules();
  let attempts = 0;
  vi.doMock("../src/core-extras.js", async (importOriginal) => {
    attempts++;
    gate(attempts);
    return importOriginal();
  });
  const [{ UndraCore }, errors, callError] = await Promise.all([import("../src/core.js"), import("../src/errors.js"), import("../src/call-error.js")]);
  return { UndraCore, errors, callError, attempts: () => attempts };
}

function scripted(): LifecycleAdapter & { emit(state: AppState): void } {
  const emitters = new Set<(state: AppState) => void>();
  return {
    subscribe(emit) {
      emitters.add(emit);
      return () => void emitters.delete(emit);
    },
    emit(state) {
      for (const emit of emitters) emit(state);
    },
  };
}

async function boot(runtime: Awaited<ReturnType<typeof fresh>>, options: { stats?: string; lifecycle?: LifecycleAdapter; mode?: string } = {}) {
  const fake = new FakeCoreTransport({ mode: options.mode ?? "remote" });
  if (options.stats !== undefined) fake.stats = () => Promise.resolve(options.stats as string);
  const core = await runtime.UndraCore.attach(fake, {
    expectedSchemaHash: SCHEMA,
    shared: false,
    adapters: { log: captureLog(), http: null, timer: null, kv: null, secureStore: null, fs: null, connectivity: null, lifecycle: options.lifecycle ?? null },
  });
  return { fake, core };
}

describe("core-extras loads on the first call", () => {
  it("imports it once for stats and runInBackground together, and not before; snapshot and restore never wait for it", async () => {
    const runtime = await fresh();
    const { core } = await boot(runtime, { stats: '{"live_handles":2}' });
    expect(runtime.attempts(), "attach imports nothing of it").toBe(0);
    await expect(core.snapshot()).rejects.toBeInstanceOf(runtime.errors.UndraModeError); // a remote core cannot snapshot
    await expect(core.restore(new Uint8Array(0))).rejects.toBeInstanceOf(runtime.errors.UndraModeError);
    expect(runtime.attempts(), "snapshot and restore are answered at the call").toBe(0);
    expect((await core.stats()).liveHandles).toBe(2);
    expect(runtime.attempts()).toBe(1);
    expect((await core.stats()).liveHandles).toBe(2);
    expect(runtime.attempts(), "the module is cached").toBe(1);
    core.close();
  });

  it("a page hidden imports nothing, with nothing to drain or with work pending (the run goes out from the first chunk)", async () => {
    vi.stubGlobal("document", { visibilityState: "visible", addEventListener() {}, removeEventListener() {} });
    const runtime = await fresh();
    const lifecycle = scripted();
    const idle = await boot(runtime, { stats: '{"background":{"pending":0}}', lifecycle, mode: "native" });
    lifecycle.emit("background");
    await macrotask();
    await macrotask();
    expect(runtime.attempts(), "nothing pending: the window did not need the chunk").toBe(0);
    expect(idle.fake.calls).toEqual([]);
    idle.core.close();

    const busyLifecycle = scripted();
    const busy = await boot(runtime, { stats: '{"background":{"pending":3}}', lifecycle: busyLifecycle, mode: "native" });
    busy.fake.on(0x0e5b14ff, (_call, r) => r.defer());
    busyLifecycle.emit("background");
    await vi.waitFor(() => expect(busy.fake.calls).toHaveLength(1));
    expect(runtime.attempts(), "a page that is being left drains without fetching a chunk").toBe(0);
    busy.core.close();
  });

  it("a stats text that is not JSON, or has no background field, is zero work pending, not an error", async () => {
    vi.stubGlobal("document", { visibilityState: "visible", addEventListener() {}, removeEventListener() {} });
    const runtime = await fresh();
    const errors: unknown[] = [];
    for (const stats of ["not json", "{}", '{"background":{"pending":"3"}}', "[1]"]) {
      const lifecycle = scripted();
      const fake = new FakeCoreTransport({ mode: "native" });
      fake.stats = () => Promise.resolve(stats);
      const core = await runtime.UndraCore.attach(fake, {
        expectedSchemaHash: SCHEMA,
        shared: false,
        onError: (error) => errors.push(error),
        adapters: { log: captureLog(), http: null, timer: null, kv: null, secureStore: null, fs: null, connectivity: null, lifecycle },
      });
      lifecycle.emit("background");
      await macrotask();
      expect(fake.calls, stats).toEqual([]);
      core.close();
    }
    expect(errors).toEqual([]);
    expect(runtime.attempts()).toBe(0);
  });
});

describe("a chunk that cannot be fetched fails typed, and the next call tries again", () => {
  const chunkError = () => new TypeError("Failed to fetch dynamically imported module: https://app.test/assets/core-extras-abc.js");

  it("stats rejects UndraTransportError('closed') with the failed import as the cause", async () => {
    const runtime = await fresh((attempt) => {
      if (attempt <= 2) throw chunkError();
    });
    const { core } = await boot(runtime, { stats: '{"live_handles":1}' });
    for (let i = 0; i < 2; i++) {
      const failure = await core.stats().catch((e: unknown) => e);
      expect(failure).toBeInstanceOf(runtime.errors.UndraTransportError);
      expect((failure as { reason: string }).reason).toBe("closed");
      expect((failure as { cause?: unknown }).cause).toBeInstanceOf(Error);
    }
    expect(runtime.attempts()).toBe(2);
    // The next call tries again, and works.
    expect((await core.stats()).liveHandles).toBe(1);
    expect(runtime.attempts()).toBe(3);
    core.close();
  });

  it("runInBackground rejects UndraCallError.Unavailable, as every failure of that call is", async () => {
    const runtime = await fresh((attempt) => {
      if (attempt === 1) throw chunkError();
    });
    const { core } = await boot(runtime, { mode: "native" });
    const failure = await core.runInBackground(1000).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(runtime.callError.UndraCallError);
    expect((failure as { kind: string }).kind).toBe("unavailable");
    core.close();
  });
});

describe("the page's own run needs no chunk", () => {
  it("a page hidden with work pending drains it even when no chunk can be fetched (offline, or a page being left)", async () => {
    vi.stubGlobal("document", { visibilityState: "visible", addEventListener() {}, removeEventListener() {} });
    const runtime = await fresh(() => {
      throw new TypeError("Failed to fetch dynamically imported module");
    });
    const lifecycle = scripted();
    const errors: Array<{ operation: string }> = [];
    const fake = new FakeCoreTransport({ mode: "native" });
    fake.stats = () => Promise.resolve('{"background":{"pending":2}}');
    fake.on(0x0e5b14ff, (_call, r) => r.defer());
    const core = await runtime.UndraCore.attach(fake, {
      expectedSchemaHash: SCHEMA,
      shared: false,
      onError: (error) => errors.push(error),
      adapters: { log: captureLog(), http: null, timer: null, kv: null, secureStore: null, fs: null, connectivity: null, lifecycle },
    });
    lifecycle.emit("background");
    await vi.waitFor(() => expect(fake.calls).toHaveLength(1));
    expect(fake.calls[0]).toMatchObject({ methodId: 0x0e5b14ff });
    expect(errors, "nothing failed: the drain did not wait for a module").toEqual([]);
    expect(runtime.attempts()).toBe(0);
    core.close();
  });
});
