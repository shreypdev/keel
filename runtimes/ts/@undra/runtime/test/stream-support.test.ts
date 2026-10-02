import { afterEach, describe, expect, it, vi } from "vitest";
import { UndraCore } from "../src/core.js";
import type { UndraCallError } from "../src/call-error.js";
import type { UndraTransportError } from "../src/errors.js";
import { type UndraFeature, streams } from "../src/stream-support.js";
import { CallTarget, codecs, decodeValue } from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA, type StreamScript } from "./support/fake-core.js";
import { captureLog, deferred, macrotask, track } from "./support/harness.js";
import { u32 } from "./support/store.js";

/*
 * Streams are a feature of the generated entry (ADR-057): `features: [streams]` has the support in the page's first chunk; a core
 * loaded without it loads the module at its first stream. The stream tests of core.test.ts run both ways; this file is about the
 * load itself, the option, and what a failed load does (R6: a typed value, and the next stream tries again).
 */

const FREE = { target: CallTarget.FreeFunction } as const;
const TICKS = 4;
const none = new Uint8Array(0);

const items =
  (...values: number[]) =>
  (): StreamScript => {
    let i = 0;
    return { next: () => (i < values.length ? { done: false, value: u32(values[i++] as number) } : { done: true }) };
  };

async function collect(source: AsyncIterable<Uint8Array>): Promise<number[]> {
  const out: number[] = [];
  for await (const item of source) out.push(decodeValue(codecs.u32, item));
  return out;
}

/** The runtime's modules again, from a fresh registry: their classes are not the ones the test file imported at the top. */
async function fresh() {
  const [{ UndraCore: core }, { UndraTransportError: transport }, { UndraCallError: call }] = await Promise.all([
    import("../src/core.js"),
    import("../src/errors.js"),
    import("../src/call-error.js"),
  ]);
  return { UndraCore: core, UndraTransportError: transport, UndraCallError: call };
}

async function boot(core: typeof UndraCore, features?: readonly UndraFeature[]) {
  const fake = new FakeCoreTransport({ mode: "remote" });
  const started = track(
    await core.attach(fake, { expectedSchemaHash: SCHEMA, shared: false, adapters: { log: captureLog(), http: null, timer: null }, ...(features && { features }) }),
  );
  return { fake, core: started };
}

afterEach(() => {
  vi.doUnmock("../src/stream-support.js");
  vi.resetModules();
});

describe("features: [streams]", () => {
  it("opens the stream synchronously when iteration starts, as before the feature existed", async () => {
    const { fake, core } = await boot(UndraCore, [streams]);
    fake.stream(TICKS, items(1, 2));
    const iterator = core.stream(FREE, TICKS, none)[Symbol.asyncIterator]();
    expect(fake.calls.map((c) => c.callId), "the Call went out inside [Symbol.asyncIterator]()").toEqual([1]);
    await iterator.return?.();
  });

  it("counts the open stream in stats().openStreams, and not after it ended", async () => {
    const { fake, core } = await boot(UndraCore, [streams]);
    fake.stream(TICKS, items(1));
    const iterator = core.stream(FREE, TICKS, none)[Symbol.asyncIterator]();
    expect((await core.stats()).openStreams).toBe(1);
    expect((await iterator.next()).done).toBe(false);
    expect((await iterator.next()).done).toBe(true);
    expect((await core.stats()).openStreams).toBe(0);
  });

  it("is installed by the core that is given it, not by loading the module: another core without the option still loads the support itself", async () => {
    const withFeature = await boot(UndraCore, [streams]);
    const without = await boot(UndraCore);
    withFeature.fake.stream(TICKS, items(7));
    without.fake.stream(TICKS, items(8));
    expect(await collect(withFeature.core.stream(FREE, TICKS, none))).toEqual([7]);
    expect(await collect(without.core.stream(FREE, TICKS, none))).toEqual([8]);
  });

  it("may be given twice, and with other features, without opening anything twice", async () => {
    const other: UndraFeature = { _install: vi.fn() };
    const { fake, core } = await boot(UndraCore, [streams, other, streams]);
    fake.stream(TICKS, items(5));
    expect(await collect(core.stream(FREE, TICKS, none))).toEqual([5]);
    expect(fake.calls).toHaveLength(1);
    expect(other._install).toHaveBeenCalledTimes(1);
  });
});

describe("a core without the feature loads the support at its first stream", () => {
  async function freshCore(gate?: Promise<void>, onLoad?: () => void) {
    vi.resetModules();
    vi.doMock("../src/stream-support.js", async (importOriginal) => {
      onLoad?.();
      await gate;
      return importOriginal();
    });
    const runtime = await fresh();
    return { ...(await boot(runtime.UndraCore)), runtime };
  }

  it("sends the Call once the module is there, and the stream then behaves as with the feature", async () => {
    const gate = deferred();
    const { fake, core } = await freshCore(gate.promise);
    fake.stream(TICKS, items(1, 2, 3));
    const iterator = core.stream(FREE, TICKS, none)[Symbol.asyncIterator]();
    await macrotask();
    expect(fake.calls, "nothing was sent while the chunk was loading").toHaveLength(0);
    const first = iterator.next();
    gate.resolve();
    expect(decodeValue(codecs.u32, (await first).value as Uint8Array)).toBe(1);
    expect(fake.calls.map((c) => c.callId)).toEqual([1]);
    expect(await collect({ [Symbol.asyncIterator]: () => iterator })).toEqual([2, 3]);
  });

  it("opens the Call as iteration starts even when nobody has called next() yet, and leaving a running stream sends a Cancel", async () => {
    const { fake, core } = await freshCore();
    let n = 0;
    fake.stream(TICKS, () => ({ next: () => ({ done: false, value: u32(n++) }) })); // never ends: it is running when we leave
    const iterator = core.stream(FREE, TICKS, none)[Symbol.asyncIterator]();
    await vi.waitFor(() => expect(fake.calls.map((c) => c.callId)).toEqual([1]));
    await iterator.return?.();
    await vi.waitFor(() => expect(fake.cancelled, "leaving early cancels the stream in the core").toEqual([1]));
  });

  it("imports the module once per core, however many streams it opens", async () => {
    let loads = 0;
    const { fake, core } = await freshCore(undefined, () => {
      loads++;
    });
    fake.stream(TICKS, items(1));
    expect(await collect(core.stream(FREE, TICKS, none))).toEqual([1]);
    expect(await collect(core.stream(FREE, TICKS, none))).toEqual([1]);
    expect(loads).toBe(1);
  });

  it("does not load it for a core that is closed: the stream fails closed at once", async () => {
    let loads = 0;
    const { core, runtime } = await freshCore(undefined, () => {
      loads++;
    });
    core.close();
    const outcome = await collect(core.stream(FREE, TICKS, none)).catch((e: unknown) => e);
    expect(outcome).toBeInstanceOf(runtime.UndraTransportError);
    expect((outcome as UndraTransportError).reason).toBe("closed");
    expect(loads).toBe(0);
  });

  it("a chunk that cannot load ends the stream with UndraTransportError('closed') and its cause, which a generated stream maps to Unavailable; the next stream tries again", async () => {
    vi.resetModules();
    let attempts = 0;
    vi.doMock("../src/stream-support.js", async (importOriginal) => {
      attempts++;
      if (attempts === 1) throw new TypeError("Failed to fetch dynamically imported module: https://app.test/assets/stream-support-abc.js");
      return importOriginal();
    });
    const runtime = await fresh();
    const { fake, core } = await boot(runtime.UndraCore);
    fake.stream(TICKS, items(4, 5));
    const failure = await collect(core.stream(FREE, TICKS, none)).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(runtime.UndraTransportError);
    expect((failure as UndraTransportError).reason).toBe("closed");
    expect((failure as UndraTransportError).cause).toBeInstanceOf(Error);
    expect(fake.calls, "nothing reached the core").toHaveLength(0);
    const mapped = runtime.UndraCallError.mappedStream(failure);
    expect(mapped).toBeInstanceOf(runtime.UndraCallError);
    expect((mapped as UndraCallError).kind).toBe("unavailable");
    // The failure is said once; the iterator is over after it.
    expect(await collect(core.stream(FREE, TICKS, none))).toEqual([4, 5]);
    expect(attempts).toBe(2);
  });

  it("a failed load is reported once to the iterator and an iterator nobody pulls from does not leave an unhandled rejection", async () => {
    vi.resetModules();
    vi.doMock("../src/stream-support.js", () => {
      throw new Error("the chunk did not load");
    });
    const unhandled = vi.fn();
    process.on("unhandledRejection", unhandled);
    try {
      const runtime = await fresh();
      const { core } = await boot(runtime.UndraCore);
      void core.stream(FREE, TICKS, none)[Symbol.asyncIterator]();
      await macrotask();
      const pulled = core.stream(FREE, TICKS, none)[Symbol.asyncIterator]();
      await expect(pulled.next()).rejects.toBeInstanceOf(runtime.UndraTransportError);
      expect(await pulled.next()).toEqual({ done: true, value: undefined });
      await macrotask();
      await macrotask();
      expect(unhandled).not.toHaveBeenCalled();
    } finally {
      process.off("unhandledRejection", unhandled);
    }
  });
});
