import { afterEach, describe, expect, it, vi } from "vitest";
import { UndraCore } from "../src/core.js";
import { PortIds } from "../src/adapters/ids.js";
import { defaultPorts } from "../src/adapters/default-ports.js";
import type { KvAdapter } from "../src/adapters/types.js";
import { PortStatus, codecs, decodeValue, encodeValue } from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { captureLog, track } from "./support/harness.js";

/*
 * The Http, Kv, SecureStore and Fs ports every core gets by default load their code on their first call (ADR-052's
 * amendment of 2026-10-02): registered at once, built once, and a call that waits for the module is an ordinary
 * asynchronous port call.
 */

afterEach(() => {
  vi.doUnmock("../src/adapters/standard.js");
  vi.resetModules();
});

const memoryKv = (): KvAdapter & { readonly data: Map<string, Uint8Array> } => {
  const data = new Map<string, Uint8Array>();
  return {
    data,
    get: (key) => Promise.resolve(data.get(key) ?? null),
    set: (key, value) => Promise.resolve(void data.set(key, value)),
    delete: (key) => Promise.resolve(void data.delete(key)),
    list: (prefix) => Promise.resolve([...data.keys()].filter((k) => k.startsWith(prefix)).sort()),
  };
};
const key = (name: string): Uint8Array => encodeValue(codecs.string, name);

describe("defaultPorts", () => {
  it("registers Kv, SecureStore and Fs always, Http where there is a fetch, each asynchronous and named", () => {
    const ports = defaultPorts(undefined);
    expect([...ports.keys()].sort()).toEqual(
      [PortIds.Kv.portId, PortIds.SecureStore.portId, PortIds.Fs.portId, ...(typeof fetch === "function" ? [PortIds.Http.portId] : [])].sort(),
    );
    for (const port of ports.values()) expect(port.sync).toBe(false);
    expect(ports.get(PortIds.Kv.portId)?.name).toBe("Kv");
    expect(Object.keys(ports.get(PortIds.Kv.portId)?.methods ?? {}).map(Number).sort()).toEqual(
      [PortIds.Kv.get, PortIds.Kv.set, PortIds.Kv.delete, PortIds.Kv.list].sort(),
    );
    expect(Object.keys(ports.get(PortIds.Fs.portId)?.methods ?? {}).map(Number).sort()).toEqual(
      [PortIds.Fs.read, PortIds.Fs.write, PortIds.Fs.delete, PortIds.Fs.list].sort(),
    );
  });

  it("leaves out the port of an adapter that is null, and keeps the others", () => {
    const ports = defaultPorts({ kv: null, http: null, fs: null });
    expect([...ports.keys()]).toEqual([PortIds.SecureStore.portId]);
  });

  it("serves the app's own adapter, building the port on the first call only", async () => {
    const kv = memoryKv();
    const port = defaultPorts({ kv }).get(PortIds.Kv.portId);
    const methods = port?.methods ?? {};
    const set = methods[PortIds.Kv.set] as (args: Uint8Array) => Promise<Uint8Array>;
    const get = methods[PortIds.Kv.get] as (args: Uint8Array) => Promise<Uint8Array>;
    const value = Uint8Array.of(1, 2, 3);
    await set(Uint8Array.from([...key("k"), ...encodeValue(codecs.bytes, value)]));
    expect(kv.data.get("k")).toEqual(value);
    expect(decodeValue(codecs.option(codecs.bytes), await get(key("k")))).toEqual(value);
  });

  it("tries again after a failed load instead of keeping the failure", async () => {
    vi.resetModules();
    let attempts = 0;
    vi.doMock("../src/adapters/standard.js", async (importOriginal) => {
      attempts++;
      if (attempts === 1) throw new Error("the chunk did not load");
      return importOriginal();
    });
    const fresh = await import("../src/adapters/default-ports.js");
    const kv = memoryKv();
    kv.data.set("k", Uint8Array.of(9));
    const get = fresh.defaultPorts({ kv }).get(PortIds.Kv.portId)?.methods[PortIds.Kv.get] as (args: Uint8Array) => Promise<Uint8Array>;
    await expect(get(key("k"))).rejects.toThrow();
    expect(decodeValue(codecs.option(codecs.bytes), await get(key("k")))).toEqual(Uint8Array.of(9));
    expect(attempts).toBe(2);
  });
});

describe("a core's default ports", () => {
  async function boot(adapters: NonNullable<Parameters<typeof UndraCore.attach>[1]["adapters"]>) {
    const fake = new FakeCoreTransport({ mode: "remote" });
    track(await UndraCore.attach(fake, { expectedSchemaHash: SCHEMA, shared: false, adapters: { log: captureLog(), ...adapters } }));
    return fake;
  }

  it("answers a call to a port the app gave an adapter for, once the code has loaded", async () => {
    const kv = memoryKv();
    kv.data.set("hello", Uint8Array.of(5, 6));
    const fake = await boot({ kv, http: null });
    const reply = await fake.callPort(PortIds.Kv.portId, PortIds.Kv.get, key("hello"));
    expect(reply.status).toBe(PortStatus.Ok);
    expect(decodeValue(codecs.option(codecs.bytes), reply.body)).toEqual(Uint8Array.of(5, 6));
  });

  it("answers 'unavailable' for a port the app removed with null", async () => {
    const fake = await boot({ kv: null, http: null });
    const reply = await fake.callPort(PortIds.Kv.portId, PortIds.Kv.get, key("hello"));
    expect(reply.status).toBe(PortStatus.Unavailable);
  });
});

describe("calls that arrive while the ports' code is loading, and a load that fails (review of ADR-052's amendment)", () => {
  const entry = (name: string, value: Uint8Array): Uint8Array => Uint8Array.from([...key(name), ...encodeValue(codecs.bytes, value)]);

  async function freshCore(adapters: Record<string, unknown>, onError: (error: unknown) => void = () => {}) {
    const fresh = await import("../src/core.js");
    const fake = new FakeCoreTransport({ mode: "remote" });
    track(
      await fresh.UndraCore.attach(fake, {
        expectedSchemaHash: SCHEMA,
        shared: false,
        adapters: { log: captureLog(), http: null, ...adapters } as never,
        onError,
      }),
    );
    return fake;
  }

  it("queues the first calls of a port behind the load, drops none, and runs them in the order they were made", async () => {
    // (Two ports' first calls were checked against a real ES module loader with a delayed chunk, by the reviewer: each port's
    // calls run in order after the one load; across ports the calls of one port run before the other's. Vitest's module mock
    // answers a second `import()` of a mocked module while the first is pending with the real module, so this test gates one.)
    vi.resetModules();
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    vi.doMock("../src/adapters/standard.js", async (importOriginal) => {
      await gate;
      return importOriginal();
    });
    const kv = memoryKv();
    const fake = await freshCore({ kv, secureStore: null, fs: null });
    const calls = [
      fake.callPort(PortIds.Kv.portId, PortIds.Kv.set, entry("a", Uint8Array.of(1))),
      fake.callPort(PortIds.Kv.portId, PortIds.Kv.set, entry("a", Uint8Array.of(3))),
      fake.callPort(PortIds.Kv.portId, PortIds.Kv.get, key("a")),
    ];
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(kv.data.size, "nothing ran before the chunk arrived, and nothing was refused").toBe(0);
    release();
    const replies = await Promise.all(calls);
    expect(replies.map((r) => r.status)).toEqual([PortStatus.Ok, PortStatus.Ok, PortStatus.Ok]);
    expect(kv.data.get("a"), "the later set won: the port's calls ran in the order they were made").toEqual(Uint8Array.of(3));
    expect(decodeValue(codecs.option(codecs.bytes), (replies[2] as { body: Uint8Array }).body)).toEqual(Uint8Array.of(3));
  });

  it("a chunk that cannot load (a CSP, an offline deploy) is answered 'unavailable', reported naming the port, and the next call loads it", async () => {
    vi.resetModules();
    let attempts = 0;
    vi.doMock("../src/adapters/standard.js", async (importOriginal) => {
      attempts++;
      if (attempts === 1) throw new TypeError("Failed to fetch dynamically imported module: https://app.test/assets/standard-abc.js");
      return importOriginal();
    });
    const reported: Array<{ message: string }> = [];
    const kv = memoryKv();
    kv.data.set("k", Uint8Array.of(7));
    const fake = await freshCore({ kv, fs: null, secureStore: null }, (error) => reported.push(error as { message: string }));
    const first = await fake.callPort(PortIds.Kv.portId, PortIds.Kv.get, key("k"));
    expect(first.status).toBe(PortStatus.Unavailable);
    expect(reported).toHaveLength(1);
    expect(reported[0]?.message).toMatch(/Kv port 0x[0-9a-f]+ method 0x/);
    // (The mock factory's failure is wrapped by vitest; a real failed `import()` carries its own TypeError's text.)
    const second = await fake.callPort(PortIds.Kv.portId, PortIds.Kv.get, key("k"));
    expect(second.status).toBe(PortStatus.Ok);
    expect(decodeValue(codecs.option(codecs.bytes), second.body)).toEqual(Uint8Array.of(7));
  });

  it("no default port is synchronous, whatever adapter backs it: a lazy chunk cannot implement a synchronous port", async () => {
    const ports = defaultPorts({ kv: memoryKv(), secureStore: memoryKv() });
    expect([...ports.values()].every((p) => p.sync === false)).toBe(true);
  });
});
