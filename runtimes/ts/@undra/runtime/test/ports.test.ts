import { describe, expect, it, vi } from "vitest";
import { PortIds } from "../src/adapters/ids.js";
import { HttpErrorCodec, HttpRequestCodec, HttpResponseCodec, NetKindCodec, AppStateCodec } from "../src/adapters/codecs.js";
import { HttpError, type Adapters, type AppState, type KvAdapter, type NetKind } from "../src/adapters/types.js";
import { UndraCore } from "../src/core.js";
import { UndraPortError } from "../src/errors.js";
import type { PortImpl } from "../src/port.js";
import { UndraReader, UndraWriter, PortStatus, codecs, decodeValue, encodeValue } from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { captureLog, microtasks, track } from "./support/harness.js";

const P = 0xabcd_0001;
const M = 0xabcd_0002;
const M2 = 0xabcd_0003;

async function setup(
  options: { mode?: string; ports?: Record<number, PortImpl>; adapters?: Partial<{ [K in keyof Adapters]: Adapters[K] | null }>; onError?: (e: unknown) => void } = {},
) {
  const fake = new FakeCoreTransport({ mode: options.mode ?? "remote" });
  const log = captureLog();
  const core = track(
    await UndraCore.attach(fake, {
      expectedSchemaHash: SCHEMA,
      shared: false,
      adapters: { log, http: null, timer: null, kv: null, secureStore: null, fs: null, connectivity: null, lifecycle: null, ...options.adapters },
      ...(options.ports && { ports: options.ports }),
      ...(options.onError && { onError: options.onError }),
    }),
  );
  return { fake, core, log };
}

describe("port dispatch", () => {
  it("answers a synchronous implementation with a sync outcome", async () => {
    const { fake } = await setup({
      ports: { [P]: { sync: true, methods: { [M]: (args) => Uint8Array.from(args).reverse() } } },
    });
    const reply = await fake.callPort(P, M, new Uint8Array([1, 2, 3]));
    expect(reply).toEqual({ portCallId: 1, status: PortStatus.Ok, body: new Uint8Array([3, 2, 1]) });
  });

  it("answers an asynchronous implementation later, with the port call id of the call", async () => {
    const { fake } = await setup({
      ports: {
        [P]: {
          sync: false,
          methods: {
            [M]: async (args) => {
              await new Promise((resolve) => setTimeout(resolve, 5));
              return args;
            },
          },
        },
      },
    });
    const first = fake.callPort(P, M, new Uint8Array([1]));
    const second = fake.callPort(P, M, new Uint8Array([2]));
    expect(await second).toEqual({ portCallId: 2, status: PortStatus.Ok, body: new Uint8Array([2]) });
    expect(await first).toEqual({ portCallId: 1, status: PortStatus.Ok, body: new Uint8Array([1]) });
  });

  it("turns UndraPortError into status 1 with the encoded error, sync or async", async () => {
    const { fake } = await setup({
      ports: {
        [P]: {
          sync: false,
          methods: {
            [M]: () => {
              throw new UndraPortError(new Uint8Array([9]));
            },
            [M2]: () => Promise.reject(new UndraPortError(new Uint8Array([8, 8]))),
          },
        },
      },
    });
    expect(await fake.callPort(P, M)).toMatchObject({ status: PortStatus.Error, body: new Uint8Array([9]) });
    expect(await fake.callPort(P, M2)).toMatchObject({ status: PortStatus.Error, body: new Uint8Array([8, 8]) });
  });

  it("logs any other failure and answers unavailable", async () => {
    const errors: unknown[] = [];
    const { fake, log } = await setup({
      onError: (e) => errors.push(e),
      ports: {
        [P]: {
          sync: false,
          methods: {
            [M]: () => {
              throw new Error("sync failure");
            },
            [M2]: () => Promise.reject(new Error("async failure")),
          },
        },
      },
    });
    expect(await fake.callPort(P, M)).toMatchObject({ status: PortStatus.Unavailable, body: new Uint8Array(0) });
    expect(await fake.callPort(P, M2)).toMatchObject({ status: PortStatus.Unavailable });
    expect(errors.map((e) => (e as Error).message)).toEqual(["sync failure", "async failure"]);
    expect(log.records.map((r) => r.level)).toEqual([4, 4]);
    expect(log.records[0]?.message).toContain("port 0xabcd0001 method 0xabcd0002: sync failure");
  });

  it("reports an unregistered port or method as unavailable", async () => {
    const { fake } = await setup({ ports: { [P]: { sync: true, methods: {} } } });
    expect((await fake.callPort(P + 1, M)).status).toBe(PortStatus.Unavailable);
    expect((await fake.callPort(P, M)).status).toBe(PortStatus.Unavailable);
  });

  it("registerPort adds and replaces ports after load", async () => {
    const { fake, core } = await setup();
    expect((await fake.callPort(P, M)).status).toBe(PortStatus.Unavailable);
    core.registerPort(P, { sync: true, methods: { [M]: () => new Uint8Array([1]) } });
    expect((await fake.callPort(P, M)).body).toEqual(new Uint8Array([1]));
    core.registerPort(P, { sync: true, methods: { [M]: () => new Uint8Array([2]) } });
    expect((await fake.callPort(P, M)).body).toEqual(new Uint8Array([2]));
  });

  it("does not send the reply of a port call that finished after the core closed", async () => {
    let finish: (() => void) | undefined;
    const { fake, core } = await setup({
      ports: {
        [P]: {
          sync: false,
          methods: {
            [M]: () =>
              new Promise<Uint8Array>((resolve) => {
                finish = () => resolve(new Uint8Array(0));
              }),
          },
        },
      },
    });
    void fake.callPort(P, M);
    await fake.settle();
    core.close();
    const sentBefore = fake.sent.length;
    (finish as () => void)();
    await microtasks();
    expect(fake.sent).toHaveLength(sentBefore);
  });
});

describe("standard ports from adapters", () => {
  it("registers Http, Kv, SecureStore and Fs from the adapters that are given", async () => {
    const seen: string[] = [];
    const kv: KvAdapter = {
      async get(key) {
        seen.push(`get ${key}`);
        return new Uint8Array([1]);
      },
      async set(key) {
        seen.push(`set ${key}`);
      },
      async delete(key) {
        seen.push(`delete ${key}`);
      },
      async list(prefix) {
        seen.push(`list ${prefix}`);
        return ["a", "b"];
      },
    };
    const { fake } = await setup({
      adapters: {
        kv,
        secureStore: { ...kv, get: async () => new Uint8Array([2]) },
        http: {
          async request(req) {
            if (req.url === "https://fail.test/") throw new HttpError.Timeout();
            return { status: 204, headers: [{ name: "x", value: "y" }], body: new Uint8Array(0) };
          },
        },
        fs: {
          read: async () => new Uint8Array([7]),
          write: async () => {},
          delete: async () => {},
          list: async () => ["f"],
        },
      },
    });
    const key = encodeValue(codecs.string, "k");
    const kvGet = await fake.callPort(PortIds.Kv.portId, PortIds.Kv.get, key);
    expect(kvGet.status).toBe(PortStatus.Ok);
    expect(decodeValue(codecs.option(codecs.bytes), kvGet.body)).toEqual(new Uint8Array([1]));
    const secureGet = await fake.callPort(PortIds.SecureStore.portId, PortIds.SecureStore.get, key);
    expect(decodeValue(codecs.option(codecs.bytes), secureGet.body)).toEqual(new Uint8Array([2]));
    expect(seen).toEqual(["get k"]);

    const request = (url: string) =>
      encodeValue(HttpRequestCodec, { method: "get", url, headers: [], body: null, timeoutMs: null });
    const ok = await fake.callPort(PortIds.Http.portId, PortIds.Http.request, request("https://ok.test/"));
    expect(decodeValue(HttpResponseCodec, ok.body)).toMatchObject({ status: 204 });
    const failed = await fake.callPort(PortIds.Http.portId, PortIds.Http.request, request("https://fail.test/"));
    expect(failed.status).toBe(PortStatus.Error);
    expect(decodeValue(HttpErrorCodec, failed.body)).toBeInstanceOf(HttpError.Timeout);

    const fsRead = await fake.callPort(PortIds.Fs.portId, PortIds.Fs.read, key);
    expect(decodeValue(codecs.bytes, fsRead.body)).toEqual(new Uint8Array([7]));
  });

  it("leaves a port unavailable when its adapter is removed", async () => {
    const { fake } = await setup();
    expect((await fake.callPort(PortIds.Kv.portId, PortIds.Kv.get, new Uint8Array(0))).status).toBe(PortStatus.Unavailable);
    expect((await fake.callPort(PortIds.Http.portId, PortIds.Http.request, new Uint8Array(0))).status).toBe(PortStatus.Unavailable);
  });

  it("explicit ports override what an adapter registered", async () => {
    const { fake } = await setup({
      adapters: { kv: { get: async () => null, set: async () => {}, delete: async () => {}, list: async () => [] } },
      ports: { [PortIds.Kv.portId]: { sync: true, methods: { [PortIds.Kv.get]: () => new Uint8Array([42]) } } },
    });
    expect((await fake.callPort(PortIds.Kv.portId, PortIds.Kv.get)).body).toEqual(new Uint8Array([42]));
  });
});

describe("Timer port for a native core", () => {
  it("is served only when a Timer adapter is given explicitly in remote mode, and fires TimerFired", async () => {
    let fire: ((id: number) => void) | undefined;
    const armed: Array<[number, number]> = [];
    const { fake } = await setup({
      mode: "remote",
      adapters: {
        timer: {
          set(id, delay, f) {
            armed.push([id, delay]);
            fire = f;
          },
        },
      },
    });
    const w = new UndraWriter();
    w.writeU32(0x8000_0002);
    w.writeU64(250n);
    const reply = await fake.callPort(PortIds.Timer.portId, PortIds.Timer.set, w.finish());
    expect(reply.status).toBe(PortStatus.Ok);
    expect(armed).toEqual([[0x8000_0002, 250]]);
    (fire as (id: number) => void)(0x8000_0002);
    expect(fake.timersFired).toEqual([0x8000_0002]);
  });

  it("is not registered for wasm modes, whose timers are the timer_set import", async () => {
    const { fake } = await setup({ mode: "wasm-worker", adapters: { timer: { set() {} } } });
    expect((await fake.callPort(PortIds.Timer.portId, PortIds.Timer.set, new Uint8Array(0))).status).toBe(PortStatus.Unavailable);
  });

  it("reports a TimerFired that cannot be sent", async () => {
    let fire: ((id: number) => void) | undefined;
    const errors: unknown[] = [];
    const { fake, core } = await setup({
      mode: "remote",
      onError: (e) => errors.push(e),
      adapters: {
        timer: {
          set(_id, _delay, f) {
            fire = f;
          },
        },
      },
    });
    const w = new UndraWriter();
    w.writeU32(1);
    w.writeU64(1n);
    await fake.callPort(PortIds.Timer.portId, PortIds.Timer.set, w.finish());
    core.close();
    (fire as (id: number) => void)(1);
    expect(errors).toHaveLength(1);
  });
});

describe("event sources", () => {
  it("connectivity and lifecycle changes reach the core as Event envelopes", async () => {
    let emitConnectivity: ((online: boolean, kind: NetKind) => void) | undefined;
    let emitLifecycle: ((state: AppState) => void) | undefined;
    const stops = { connectivity: vi.fn(), lifecycle: vi.fn() };
    const { fake, core } = await setup({
      adapters: {
        connectivity: {
          subscribe(emit) {
            emitConnectivity = emit;
            return stops.connectivity;
          },
        },
        lifecycle: {
          subscribe(emit) {
            emitLifecycle = emit;
            return stops.lifecycle;
          },
        },
      },
    });
    (emitConnectivity as (o: boolean, k: NetKind) => void)(true, "wifi");
    (emitLifecycle as (s: AppState) => void)("background");
    expect(fake.events).toHaveLength(2);
    const [connectivity, lifecycle] = fake.events;
    expect([connectivity?.portId, connectivity?.methodId]).toEqual([PortIds.Connectivity.portId, PortIds.Connectivity.changed]);
    const r = new UndraReader(connectivity?.payload as Uint8Array);
    expect([r.readBool(), NetKindCodec.decode(r)]).toEqual([true, "wifi"]);
    r.finish();
    expect([lifecycle?.portId, lifecycle?.methodId]).toEqual([PortIds.Lifecycle.portId, PortIds.Lifecycle.changed]);
    expect(decodeValue(AppStateCodec, lifecycle?.payload as Uint8Array)).toBe("background");
    core.close();
    expect(stops.connectivity).toHaveBeenCalledOnce();
    expect(stops.lifecycle).toHaveBeenCalledOnce();
  });

  it("an event that cannot be sent is reported, not thrown into the platform's event loop", async () => {
    let emit: ((o: boolean, k: NetKind) => void) | undefined;
    const errors: unknown[] = [];
    const { core } = await setup({
      onError: (e) => errors.push(e),
      adapters: {
        connectivity: {
          subscribe(e) {
            emit = e;
            return () => {};
          },
        },
      },
    });
    core.close();
    expect(() => (emit as (o: boolean, k: NetKind) => void)(false, "none")).not.toThrow();
    expect(errors).toHaveLength(1);
  });
});
