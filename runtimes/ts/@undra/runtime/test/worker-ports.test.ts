import { describe, expect, it } from "vitest";
import { PortIds } from "../src/adapters/ids.js";
import { UndraCore } from "../src/core.js";
import { UndraReplyError, UndraSchemaMismatchError } from "../src/errors.js";
import type { PortImpl } from "../src/port.js";
import { WasmWorkerTransport, type WorkerLike } from "../src/transport/wasm-worker.js";
import { runWorker, type WorkerScope } from "../src/worker.js";
import {
  CallTarget,
  Kind,
  PortStatus,
  ReplyStatus,
  decodeEnvelope,
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
 * The ports of `wasm-worker` mode (docs/SPEC.md sections 7 and 17.1). The core calls Clock, Rng and Log
 * synchronously and cannot wait for the main thread, so the worker must answer "unavailable" to the
 * `port_call` import for them (the wasm shell then answers from its built-in bindings over the `now_ms`,
 * `random` and `log` imports); answering "async" left `port_call_sync` without an answer and the infallible
 * proxy panicked, which on wasm is a trap that kills the core (gap PO-4). Every other port crosses to the
 * main thread. The real core in a real worker thread is covered by crates/undra-ffi/tests/wasm.
 */

const SCHEMA = STUB.SCHEMA_HASH;

/** One envelope the worker posted. */
interface Posted {
  readonly kind: Kind;
  readonly payload: Uint8Array;
}

/** A worker scope driven by hand: the test delivers messages and reads the envelopes the worker posted. */
async function driven(stub: Parameters<typeof compileStub>[0] = {}) {
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
  deliver({ t: "init", wasm: { kind: "module", module }, expectedSchemaHash: SCHEMA, platform: "test", devtools: false, logLevel: 2, protocol: 2 });
  for (let i = 0; i < 100 && !messages.some((m) => m.t === "ready"); i++) await new Promise((resolve) => setTimeout(resolve, 1));
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
] as const;

describe("the worker answers the built-in sync ports itself", () => {
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

  it("a Clock the main thread registered is not asked: the worker's built-in binding serves the core", async () => {
    let asked = 0;
    const clock: PortImpl = {
      sync: true,
      methods: {
        [STUB.PORT_METHOD]: () => {
          asked++;
          return u32(1);
        },
      },
    };
    const w = await overChannel({ portId: PortIds.Clock.portId }, { [PortIds.Clock.portId]: clock });
    // The stub turns "unavailable" into status 5; the main-thread Clock was never consulted.
    await expect(call(w.core)).rejects.toSatisfy((e: unknown) => e instanceof UndraReplyError && e.status === ReplyStatus.BadRequest);
    expect(asked).toBe(0);
    w.close();
  });

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

  it("a port declared sync cannot serve the core in worker mode: a warning names it once", async () => {
    const sum: PortImpl = { sync: true, methods: { [STUB.PORT_METHOD]: (args) => args } };
    const w = await overChannel({}, { [STUB.PORT_ID]: sum });
    await call(w.core);
    await call(w.core);
    const warnings = w.log.records.filter((r) => r.level === 3 && r.target === "undra::worker");
    expect(warnings, "once per port, however often it is called").toHaveLength(1);
    expect(warnings[0]?.message).toContain("0xc0dec0de");
    expect(warnings[0]?.message).toContain("wasm-worker");
    expect(warnings[0]?.message).toContain("wasm-main");
    w.close();
  });
});
