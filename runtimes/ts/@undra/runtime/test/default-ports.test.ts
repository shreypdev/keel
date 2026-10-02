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
