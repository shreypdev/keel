import { describe, expect, it } from "vitest";
import { PortIds } from "../src/adapters/ids.js";
import { UndraCore } from "../src/core.js";
import { UndraError, UndraPortError, UndraReplyError, UndraSchemaMismatchError } from "../src/errors.js";
import type { PortImpl } from "../src/port.js";
import { dbPort } from "../src/db.js";
import { ssePort, webSocketPort } from "../src/realtime.js";
import { WasmWorkerTransport, type WorkerLike } from "../src/transport/wasm-worker.js";
import { runWorker, type WorkerScope } from "../src/worker.js";
import {
  CallTarget,
  Kind,
  PortStatus,
  ReplyStatus,
  decodeEnvelope,
  decodeLog,
  decodePortCall,
  decodeReply,
  encodeCall,
  encodeEnvelope,
  encodePortReply,
} from "../src/wire/index.js";
import { STUB, compileStub } from "./support/stub-core.js";
import { captureLog, track } from "./support/harness.js";
import { channelWorker } from "./support/worker.js";
import { u32 } from "./support/store.js";

/*
 * The ports of `wasm-worker` mode (docs/SPEC.md sections 7 and 17.1, ADR-049). The core calls synchronous ports
 * while it runs and cannot wait for the main thread, so the worker answers what it can itself: a port implemented
 * in the worker (the module of `LoadOptions.worker.ports`) there; a port the host serves asynchronously (worker
 * protocol 3's `asyncPorts`) crosses to the main thread; anything else is "unavailable" to the `port_call` import,
 * which makes the wasm shell answer Clock, Rng and Log from its built-in bindings over the `now_ms`, `random` and
 * `log` imports. A worker that answered "async" for those left `port_call_sync` without an answer and the infallible
 * proxy panicked, which on wasm is a trap that kills the core (gap PO-4). A host before protocol 3 sends no
 * `asyncPorts`, and every port but those three crosses. The real core in a real worker thread is covered by
 * crates/undra-ffi/tests/wasm.
 */

const SCHEMA = STUB.SCHEMA_HASH;

/** One envelope the worker posted. */
interface Posted {
  readonly kind: Kind;
  readonly payload: Uint8Array;
}

/** A worker scope driven by hand: the test delivers messages and reads the envelopes the worker posted. */
async function driven(stub: Parameters<typeof compileStub>[0] = {}, init: { protocol?: number; asyncPorts?: number[]; portsModule?: string } = { protocol: 2 }) {
  const module = await WebAssembly.compile((await compileStub(stub)) as Uint8Array<ArrayBuffer>);
  const listeners: Array<(event: Event) => void> = [];
  const messages: Array<{ t: string; data?: unknown }> = [];
  const scope: WorkerScope = {
    addEventListener: (_type: string, fn: EventListenerOrEventListenerObject | null) => {
      listeners.push(fn as (event: Event) => void);
    },
    removeEventListener: () => {},
    postMessage: (message: unknown) => {
      messages.push(message as { t: string });
    },
  };
  const deliver = (message: unknown): void => {
    for (const fn of listeners) fn({ data: message } as MessageEvent);
  };
  const stop = runWorker(scope);
  deliver({ t: "init", wasm: { kind: "module", module }, expectedSchemaHash: SCHEMA, platform: "test", devtools: false, logLevel: 2, ...init });
  for (let i = 0; i < 200 && !messages.some((m) => m.t === "ready" || m.t === "failed"); i++) await new Promise((resolve) => setTimeout(resolve, 1));
  const failed = messages.find((m) => m.t === "failed") as { failure?: { message?: string } } | undefined;
  expect(failed?.failure?.message).toBeUndefined();
  expect(messages.some((m) => m.t === "ready")).toBe(true);
  messages.length = 0;

  /** The envelopes posted since the last call, in order. */
  const envelopes = async (): Promise<Posted[]> => {
    await Promise.resolve(); // the worker flushes a task's envelopes from a microtask
    const out: Posted[] = [];
    for (const message of messages.splice(0)) {
      const buffers = message.t === "envelopes" ? (message.data as ArrayBuffer[]) : message.t === "envelope" ? [message.data as ArrayBuffer] : [];
      for (const buffer of buffers) {
        const envelope = decodeEnvelope(new Uint8Array(buffer), SCHEMA);
        out.push({ kind: envelope.kind, payload: envelope.payload });
      }
    }
    return out;
  };
  return { deliver, envelopes, stop };
}

/** A `Call` envelope for the stub's PORT method: the core calls the port the stub was built with. */
const portCall = (): ArrayBuffer =>
  encodeEnvelope(Kind.Call, 0, SCHEMA, encodeCall({ target: CallTarget.FreeFunction, methodId: STUB.PORT, callId: 9, args: u32(5) })).buffer as ArrayBuffer;

const STANDARD = [
  ["Clock", PortIds.Clock.portId],
  ["Rng", PortIds.Rng.portId],
  ["Log", PortIds.Log.portId],
] as const;

const CROSSING = [
  ["a custom port", STUB.PORT_ID],
  ["Http", PortIds.Http.portId],
  ["Kv", PortIds.Kv.portId],
  ["SecureStore", PortIds.SecureStore.portId],
  ["Fs", PortIds.Fs.portId],
  ["Timer", PortIds.Timer.portId],
  // The opt-in ports of ADR-047 and ADR-048 are asynchronous: their bindings run on the main thread (ADR-049 §2).
  ["WebSocket", PortIds.WebSocket.portId],
  ["Sse", PortIds.Sse.portId],
  ["Db", PortIds.Db.portId],
] as const;

describe("a host before protocol 3: the worker answers the built-in sync ports itself, every other port crosses", () => {
  it.each(STANDARD)("%s: 'unavailable' to the core at once (so its built-in binding answers), nothing posted to the main thread", async (_name, portId) => {
    const w = await driven({ portId });
    w.deliver({ t: "envelope", data: portCall() });
    const posted = await w.envelopes();
    expect(posted.filter((p) => p.kind === Kind.PortCall), "no PortCall crosses to the main thread").toEqual([]);
    // The stub reports what `port_call` returned: 2 ("unavailable") is its status 5 "port unavailable"; 1 ("async") would have left the call waiting.
    expect(posted.map((p) => p.kind)).toEqual([Kind.Reply]);
    const reply = decodeReply((posted[0] as Posted).payload);
    expect(reply.callId).toBe(9);
    expect(reply.status).toBe(ReplyStatus.BadRequest);
    w.stop();
  });

  it.each(CROSSING)("%s: crosses to the main thread, and the core waits for the PortReply", async (_name, portId) => {
    const w = await driven({ portId });
    w.deliver({ t: "envelope", data: portCall() });
    const posted = await w.envelopes();
    expect(posted.map((p) => p.kind), "one PortCall and no reply yet").toEqual([Kind.PortCall]);
    const call = decodePortCall((posted[0] as Posted).payload);
    expect(call.portId).toBe(portId);
    expect(call.methodId).toBe(STUB.PORT_METHOD);
    // The main thread's answer completes the core's call.
    const reply = encodePortReply({ portCallId: call.portCallId, status: PortStatus.Ok, body: u32(7) });
    w.deliver({ t: "envelope", data: encodeEnvelope(Kind.PortReply, 1, SCHEMA, reply).buffer });
    const after = await w.envelopes();
    expect(after.map((p) => p.kind)).toEqual([Kind.Reply]);
    expect(decodeReply((after[0] as Posted).payload).status).toBe(ReplyStatus.Ok);
    w.stop();
  });
});

/** The URL of one of the test modules of `LoadOptions.worker.ports` (test/support/worker-ports). */
const portsModule = (name: string): string => new URL(`./support/worker-ports/${name}.mjs`, import.meta.url).href;

/** The PortReply the stub replied with: `port_call_id u32, status u8, body`. */
function stubReply(posted: Posted[]): { status: number; port: { status: number; body: number[] } } {
  const reply = decodeReply((posted.find((p) => p.kind === Kind.Reply) as Posted).payload);
  const body = reply.status === ReplyStatus.Ok ? [...(reply as { body: Uint8Array }).body] : [];
  return { status: reply.status, port: { status: body[4] ?? -1, body: body.slice(5) } };
}

describe("protocol 3: the worker answers a port where it is implemented (ADR-049)", () => {
  it.each([...STANDARD, ["a custom port", STUB.PORT_ID], ["Timer", PortIds.Timer.portId], ["Http", PortIds.Http.portId]] as const)(
    "%s, which the host does not serve asynchronously: 'unavailable' at once, nothing posted",
    async (_name, portId) => {
      const w = await driven({ portId }, { protocol: 3, asyncPorts: [PortIds.Kv.portId] });
      w.deliver({ t: "envelope", data: portCall() });
      const posted = await w.envelopes();
      expect(posted.map((p) => p.kind)).toEqual([Kind.Reply]);
      expect(decodeReply((posted[0] as Posted).payload).status, "the stub's status for an unavailable port").toBe(ReplyStatus.BadRequest);
      w.stop();
    },
  );

  it.each([["Kv", PortIds.Kv.portId], ["a custom async port", STUB.PORT_ID]] as const)(
    "%s, served by the host asynchronously: crosses to the main thread",
    async (_name, portId) => {
      const w = await driven({ portId }, { protocol: 3, asyncPorts: [portId] });
      w.deliver({ t: "envelope", data: portCall() });
      const posted = await w.envelopes();
      expect(posted.map((p) => p.kind)).toEqual([Kind.PortCall]);
      expect(decodePortCall((posted[0] as Posted).payload).portId).toBe(portId);
      w.stop();
    },
  );

  it("a `ports` message keeps the set current: a port registered after load crosses from then on", async () => {
    const w = await driven({}, { protocol: 3, asyncPorts: [] });
    w.deliver({ t: "envelope", data: portCall() });
    expect((await w.envelopes()).map((p) => p.kind)).toEqual([Kind.Reply]);
    w.deliver({ t: "ports", asyncPorts: [STUB.PORT_ID] });
    w.deliver({ t: "envelope", data: portCall() });
    expect((await w.envelopes()).map((p) => p.kind)).toEqual([Kind.PortCall]);
    w.stop();
  });

  it("a synchronous port of the worker ports module is answered in the worker, synchronously, and nothing crosses", async () => {
    const module = (await import(portsModule("sync"))) as { asked: string[] };
    module.asked.length = 0;
    const w = await driven({}, { protocol: 3, asyncPorts: [STUB.PORT_ID], portsModule: portsModule("sync") });
    w.deliver({ t: "envelope", data: portCall() });
    const posted = await w.envelopes();
    expect(posted.map((p) => p.kind), "answered inside the worker: the stub replied at once").toEqual([Kind.Reply]);
    expect(stubReply(posted)).toEqual({ status: ReplyStatus.Ok, port: { status: PortStatus.Ok, body: [9, 0, 0, 0, ...u32(5)] } });
    expect(module.asked).toEqual(["custom"]);
    w.stop();
  });

  it("the worker ports module overrides Clock: the core's Clock call is answered by it, not by the built-in", async () => {
    const module = (await import(portsModule("sync"))) as { asked: string[] };
    module.asked.length = 0;
    const w = await driven({ portId: PortIds.Clock.portId }, { protocol: 3, asyncPorts: [], portsModule: portsModule("sync") });
    w.deliver({ t: "envelope", data: portCall() });
    const posted = await w.envelopes();
    expect(stubReply(posted).port).toEqual({ status: PortStatus.Ok, body: [4, 0, 0, 0, ...u32(5)] });
    expect(module.asked).toEqual(["clock"]);
    w.stop();
  });

  it("the module's adapters back the now_ms and random imports in the worker", async () => {
    const w = await driven({}, { protocol: 3, asyncPorts: [], portsModule: portsModule("sync") });
    w.deliver({
      t: "envelope",
      data: encodeEnvelope(Kind.Call, 0, SCHEMA, encodeCall({ target: CallTarget.FreeFunction, methodId: STUB.RANDOM_NOW, callId: 4, args: new Uint8Array(0) })).buffer,
    });
    const reply = decodeReply(((await w.envelopes()).find((p) => p.kind === Kind.Reply) as Posted).payload);
    expect(reply.status).toBe(ReplyStatus.Ok);
    const body = (reply as { body: Uint8Array }).body;
    expect([...body.subarray(0, 8)]).toEqual([7, 7, 7, 7, 7, 7, 7, 7]);
    expect(new DataView(body.buffer, body.byteOffset + 8, 8).getFloat64(0, true)).toBe(4321.5);
    w.stop();
  });

  it("an asynchronous port of the module answers later, inside the worker", async () => {
    const w = await driven({}, { protocol: 3, asyncPorts: [], portsModule: portsModule("async") });
    w.deliver({ t: "envelope", data: portCall() });
    expect((await w.envelopes()).map((p) => p.kind), "the core waits").toEqual([]);
    await new Promise((resolve) => setTimeout(resolve, 20));
    const posted = await w.envelopes();
    expect(stubReply(posted).port).toEqual({ status: PortStatus.Ok, body: [3, 0, 0, 0, ...u32(5)] });
    w.stop();
  });

  it("a module port's typed failure is status 1; an untyped one is status 2 with an error-level log naming the port", async () => {
    const w = await driven({}, { protocol: 3, asyncPorts: [], portsModule: portsModule("failing") });
    w.deliver({ t: "envelope", data: portCall() });
    expect(stubReply(await w.envelopes()).port).toEqual({ status: PortStatus.Error, body: [1, 0] });
    w.deliver({ t: "envelope", data: portCall() });
    const posted = await w.envelopes();
    expect(stubReply(posted).port.status).toBe(PortStatus.Unavailable);
    const logs = posted.filter((p) => p.kind === Kind.Log).map((p) => decodeLog(p.payload));
    expect(logs.some((l) => l.level === 4 && l.message.includes("port 0xc0dec0de method 0xdeadbeef") && l.message.includes("a bug in the worker's port"))).toBe(true);
    w.stop();
  });

  it.each([
    ["does not exist", "./support/worker-ports/missing.mjs", /could not import the ports module/],
    ["does not export port implementations", "./support/worker-ports/broken.mjs", /not a port implementation/],
  ])("a ports module that %s fails the start, naming it", async (_name, path, message) => {
    const module = await WebAssembly.compile((await compileStub()) as Uint8Array<ArrayBuffer>);
    const posted: Array<{ t: string; failure?: { kind: string; reason?: string; message: string } }> = [];
    const listeners: Array<(event: Event) => void> = [];
    const stop = runWorker({
      addEventListener: (_type: string, fn: EventListenerOrEventListenerObject | null) => {
        listeners.push(fn as (event: Event) => void);
      },
      removeEventListener: () => {},
      postMessage: (message: unknown) => {
        posted.push(message as (typeof posted)[number]);
      },
    });
    const url = new URL(path, import.meta.url).href;
    for (const fn of listeners) {
      fn({ data: { t: "init", wasm: { kind: "module", module }, expectedSchemaHash: SCHEMA, platform: "test", devtools: false, logLevel: 2, protocol: 3, asyncPorts: [], portsModule: url } } as MessageEvent);
    }
    for (let i = 0; i < 200 && !posted.some((m) => m.t === "failed"); i++) await new Promise((resolve) => setTimeout(resolve, 1));
    const failed = posted.find((m) => m.t === "failed");
    expect(failed?.failure).toMatchObject({ kind: "transport", reason: "handshake" });
    expect(failed?.failure?.message).toMatch(message);
    expect(failed?.failure?.message).toContain(url);
    stop();
  });
});

describe("what the core says while it initialises", () => {
  it("every envelope the worker posts, before `ready` too, carries the schema hash the host expects", async () => {
    // A core talks during `undra_init` (a log record, the port call of an init hook that reads the cache), before the
    // worker has told the host it is ready; the host rejects an envelope of another schema.
    const module = await WebAssembly.compile((await compileStub({ logOnInit: true })) as Uint8Array<ArrayBuffer>);
    const listeners: Array<(event: Event) => void> = [];
    const messages: Array<{ t: string; data?: unknown }> = [];
    const stop = runWorker({
      addEventListener: (_type: string, fn: EventListenerOrEventListenerObject | null) => {
        listeners.push(fn as (event: Event) => void);
      },
      removeEventListener: () => {},
      postMessage: (message: unknown) => {
        messages.push(message as { t: string });
      },
    });
    for (const fn of listeners) {
      fn({ data: { t: "init", wasm: { kind: "module", module }, expectedSchemaHash: SCHEMA, platform: "test", devtools: false, logLevel: 2, protocol: 2 } } as MessageEvent);
    }
    for (let i = 0; i < 100 && !messages.some((m) => m.t === "ready"); i++) await new Promise((resolve) => setTimeout(resolve, 1));
    const ready = messages.findIndex((m) => m.t === "ready");
    expect(ready).toBeGreaterThan(0);
    const before = messages.slice(0, ready).flatMap((m) => (m.t === "envelopes" ? (m.data as ArrayBuffer[]) : m.t === "envelope" ? [m.data as ArrayBuffer] : []));
    expect(before.length, "the core's log record came before ready").toBeGreaterThan(0);
    for (const buffer of before) {
      expect(() => decodeEnvelope(new Uint8Array(buffer), SCHEMA)).not.toThrow();
      expect(decodeEnvelope(new Uint8Array(buffer), SCHEMA).kind).toBe(Kind.Log);
    }
    stop();
  });

  it("a worker that talks during init loads, and its record reaches the Log adapter", async () => {
    const module = await WebAssembly.compile((await compileStub({ logOnInit: true })) as Uint8Array<ArrayBuffer>);
    const worker = channelWorker();
    const log = captureLog();
    const core = track(
      await UndraCore.attach(new WasmWorkerTransport({ wasm: module, expectedSchemaHash: SCHEMA, worker: worker.host, startTimeoutMs: 2000 }), {
        expectedSchemaHash: SCHEMA,
        shared: false,
        adapters: { log, http: null, timer: null },
      }),
    );
    expect(log.records.filter((r) => r.level === 3 && r.message === "unknown method")).toHaveLength(1);
    core.close();
    worker.close();
  });

  it("a port call the core makes while it initialises is answered through the main thread, and the answer reaches the core", async () => {
    // Query hydration is an init hook that calls the Kv port before `undra_init` returns. The host's answer is sent
    // before the worker's `ready` has been read, which `send` used to refuse ("the core is not started").
    const module = await WebAssembly.compile((await compileStub({ portOnInit: true })) as Uint8Array<ArrayBuffer>);
    const worker = channelWorker();
    const asked: Uint8Array[] = [];
    const errors: unknown[] = [];
    const ping: PortImpl = {
      sync: false,
      methods: {
        [STUB.PORT_METHOD]: async (args) => {
          asked.push(args);
          return u32(5);
        },
      },
    };
    const core = track(
      await UndraCore.attach(new WasmWorkerTransport({ wasm: module, expectedSchemaHash: SCHEMA, worker: worker.host, startTimeoutMs: 2000 }), {
        expectedSchemaHash: SCHEMA,
        shared: false,
        adapters: { log: { log() {} }, http: null, timer: null },
        ports: { [STUB.PORT_ID]: ping },
        onError: (error) => {
          errors.push(error);
        },
      }),
    );
    expect(asked, "the main thread ran the port").toHaveLength(1);
    // What the core heard back, by the stub's INIT_PORT_REPLY method: PortReply { id 1, Ok, u32 5 }.
    const heard = await core.call(CallTarget.FreeFunction, STUB.INIT_PORT_REPLY, new Uint8Array(0));
    expect([...heard]).toEqual([1, 0, 0, 0, PortStatus.Ok, ...u32(5)]);
    expect(errors, "no failure was reported").toEqual([]);
    core.close();
    worker.close();
  });

  it("a protocol failure before `ready` fails the start at once instead of waiting for the timeout", async () => {
    // A hand-rolled worker whose first message is an envelope of another schema, then nothing.
    const listeners: Array<(event: Event) => void> = [];
    const worker: WorkerLike = {
      addEventListener: (type, fn) => {
        if (type === "message") listeners.push(fn as (event: Event) => void);
      },
      removeEventListener: () => {},
      postMessage: (message) => {
        if ((message as { t: string }).t !== "init") return;
        queueMicrotask(() => {
          const stray = encodeEnvelope(Kind.Log, 0, SCHEMA ^ 1n, new Uint8Array(0)).buffer;
          for (const fn of listeners) fn({ data: { t: "envelope", data: stray } } as MessageEvent);
        });
      },
      close: () => {},
    };
    const transport = new WasmWorkerTransport({ wasm: new Uint8Array(8), expectedSchemaHash: SCHEMA, worker, startTimeoutMs: 60_000 });
    const started = Date.now();
    const failure = await transport
      .start({ reply: () => {}, changeSet: () => {}, streamItem: () => {}, portCall: () => ({ kind: "unavailable" }), log: () => {}, closed: () => {} })
      .then(
        () => undefined,
        (e: unknown) => e,
      );
    expect(failure).toBeInstanceOf(UndraSchemaMismatchError);
    expect(Date.now() - started).toBeLessThan(5_000);
  });
});

describe("over a real channel, with UndraCore on the main thread", () => {
  async function overChannel(stub: Parameters<typeof compileStub>[0], ports: Readonly<Record<number, PortImpl>>) {
    const module = await WebAssembly.compile((await compileStub(stub)) as Uint8Array<ArrayBuffer>);
    const worker = channelWorker();
    const log = captureLog();
    const transport = new WasmWorkerTransport({ wasm: module, expectedSchemaHash: SCHEMA, worker: worker.host });
    const core = track(
      await UndraCore.attach(transport, { expectedSchemaHash: SCHEMA, shared: false, adapters: { log, http: null, timer: null }, ports }),
    );
    return {
      core,
      log,
      close() {
        core.close();
        worker.close();
      },
    };
  }

  const call = (core: UndraCore) => core.call(CallTarget.FreeFunction, STUB.PORT, u32(5));

  it("an async port crosses to the main thread and answers there", async () => {
    let asked = 0;
    const echo: PortImpl = {
      sync: false,
      methods: {
        [STUB.PORT_METHOD]: async (args) => {
          asked++;
          await Promise.resolve();
          return args;
        },
      },
    };
    const w = await overChannel({}, { [STUB.PORT_ID]: echo });
    const reply = await call(w.core);
    expect(asked).toBe(1);
    // The stub replies with the complete PortReply payload it got back (id 1, Ok, the echoed args).
    expect([...reply.subarray(4)]).toEqual([PortStatus.Ok, ...u32(5)]);
    expect(w.log.records.filter((r) => r.level >= 3)).toEqual([]);
    w.close();
  });

  it("the realtime and db bindings are asynchronous ports, so worker mode serves them on the main thread", async () => {
    // Worker mode refuses a sync port on the main thread (ADR-049); every binding method answers with a promise,
    // its typed failures included (here: closing an id that was never opened).
    const unknownClose: Array<[string, PortImpl, number, Uint8Array]> = [
      ["WebSocket", webSocketPort({ connect: () => Promise.reject(new Error("unused")) }), PortIds.WebSocket.close, Uint8Array.of(9, 0, 0, 0, 0xe8, 0x03, 0, 0, 0, 0)],
      ["Sse", ssePort({ open: () => Promise.reject(new Error("unused")) }), PortIds.Sse.close, u32(9)],
      ["Db", dbPort({ open: () => Promise.reject(new Error("unused")) }), PortIds.Db.close, u32(9)],
    ];
    for (const [name, impl, method, args] of unknownClose) {
      expect(impl.sync, name).toBe(false);
      const reply = (impl.methods[method] as (args: Uint8Array) => Uint8Array | Promise<Uint8Array>)(args);
      expect(reply, name).toBeInstanceOf(Promise);
      await expect(reply, name).rejects.toBeInstanceOf(UndraPortError);
    }
  });

  it("LoadOptions.ports with the realtime and db bindings in worker mode: the core's call crosses to the binding and its typed answer comes back", async () => {
    // The stub core calls (portId, STUB.PORT_METHOD); each binding answers it with its own `close` of an id that was never
    // opened, so the reply is the binding's typed error (status 1), produced on the main thread.
    const cases: Array<[string, PortImpl, number, Uint8Array, number]> = [
      ["WebSocket", webSocketPort({ connect: () => Promise.reject(new Error("unused")) }), PortIds.WebSocket.close, Uint8Array.of(9, 0, 0, 0, 0xe8, 0x03, 0, 0, 0, 0), PortIds.WebSocket.portId],
      ["Sse", ssePort({ open: () => Promise.reject(new Error("unused")) }), PortIds.Sse.close, u32(9), PortIds.Sse.portId],
      ["Db", dbPort({ open: () => Promise.reject(new Error("unused")) }), PortIds.Db.close, u32(9), PortIds.Db.portId],
    ];
    for (const [name, binding, method, args, portId] of cases) {
      const close = binding.methods[method] as (args: Uint8Array) => Uint8Array | Promise<Uint8Array>;
      const impl: PortImpl = { ...binding, methods: { ...binding.methods, [STUB.PORT_METHOD]: () => close(args) } };
      const w = await overChannel({ portId }, { [portId]: impl });
      const reply = await call(w.core);
      expect(reply[4], `${name}: the binding's typed error`).toBe(PortStatus.Error);
      expect(w.log.records.filter((r) => r.level >= 4), name).toEqual([]);
      w.close();
    }
  });

  it.each([
    ["an app's sync port without a name", STUB.PORT_ID, "port 0xc0dec0de", undefined],
    ["a Clock (clockPort names it)", PortIds.Clock.portId, "Clock port 0xcd99c48e", "Clock"],
  ] as const)("%s registered on the main thread is a load-time error that names it and the fix", async (_name, portId, named, portName) => {
    const module = await WebAssembly.compile((await compileStub({})) as Uint8Array<ArrayBuffer>);
    const worker = channelWorker();
    let started = false;
    const host: WorkerLike = { ...worker.host, postMessage: (m, t) => ((started = true), worker.host.postMessage(m, t)) };
    const sync: PortImpl = { ...(portName !== undefined && { name: portName }), sync: true, methods: { [STUB.PORT_METHOD]: (args) => args } };
    const failure = await UndraCore.attach(new WasmWorkerTransport({ wasm: module, expectedSchemaHash: SCHEMA, worker: host }), {
      expectedSchemaHash: SCHEMA,
      shared: false,
      adapters: { log: captureLog(), http: null, timer: null },
      ports: { [portId]: sync },
    }).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(failure).toBeInstanceOf(UndraError);
    expect((failure as UndraError).kind).toBe("options");
    expect((failure as UndraError).message).toContain(named);
    expect((failure as UndraError).message).toContain("LoadOptions.worker.ports");
    expect(started, "the worker was never sent anything").toBe(false);
    worker.close();
  });

  it("the refusal uses the port's name when its adapter carries one (generated adapters do)", async () => {
    const w = await overChannel({}, {});
    expect(() => w.core.registerPort(STUB.PORT_ID, { name: "Locale", sync: true, methods: {} })).toThrow(
      "Locale port 0xc0dec0de is synchronous",
    );
    w.close();
  });

  it("registerPort of a sync port after load throws, naming it; an async one is announced to the worker and crosses", async () => {
    const w = await overChannel({}, {});
    expect(() => w.core.registerPort(STUB.PORT_ID, { sync: true, methods: { [STUB.PORT_METHOD]: (args) => args } })).toThrow(/worker\.ports/);
    // Not registered: the core still finds the port unavailable.
    await expect(call(w.core)).rejects.toSatisfy((e: unknown) => e instanceof UndraReplyError && e.status === ReplyStatus.BadRequest);
    let asked = 0;
    w.core.registerPort(STUB.PORT_ID, {
      sync: false,
      methods: {
        [STUB.PORT_METHOD]: async (args) => {
          asked++;
          return args;
        },
      },
    });
    const reply = await call(w.core);
    expect(asked).toBe(1);
    expect([...reply.subarray(4)]).toEqual([PortStatus.Ok, ...u32(5)]);
    w.close();
  });

  it("an explicit Clock, Rng or Timer adapter is said, once, not to reach the worker", async () => {
    const module = await WebAssembly.compile((await compileStub({})) as Uint8Array<ArrayBuffer>);
    const worker = channelWorker();
    const log = captureLog();
    const core = track(
      await UndraCore.attach(new WasmWorkerTransport({ wasm: module, expectedSchemaHash: SCHEMA, worker: worker.host }), {
        expectedSchemaHash: SCHEMA,
        shared: false,
        adapters: { log, http: null, clock: { nowMs: () => 1, monotonicNs: () => 1n }, timer: { set() {} } },
      }),
    );
    const warnings = log.records.filter((r) => r.target === "undra::worker" && r.level === 3);
    expect(warnings).toHaveLength(1);
    expect(warnings[0]?.message).toContain("adapters.clock, adapters.timer");
    expect(warnings[0]?.message).toContain("LoadOptions.worker.ports");
    core.close();
    worker.close();
  });
});
