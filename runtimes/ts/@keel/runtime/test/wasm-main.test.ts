import { describe, expect, it, vi } from "vitest";
import { KeelCore } from "../src/core.js";
import { KeelModeError, KeelPortError, KeelReplyError, KeelSchemaMismatchError, KeelTransportError } from "../src/errors.js";
import type { PortImpl } from "../src/port.js";
import { WasmMainTransport } from "../src/transport/wasm-main.js";
import {
  ALL_SIGNALS,
  CallTarget,
  ChangeOp,
  KeelReader,
  KeelWriter,
  Kind,
  PortStatus,
  ReplyStatus,
  decodePortReply,
  encodeCall,
  encodeCancel,
  encodeHello,
  encodeStreamCredit,
} from "../src/wire/index.js";
import { assemble, compileStub, stubGlobals, STUB } from "./support/stub-core.js";
import { bytesOf, captureLog, deferred, macrotask, microtasks, track, utf8 } from "./support/harness.js";

const SCHEMA = STUB.SCHEMA_HASH;
const FREE = { target: CallTarget.FreeFunction } as const;

interface Boot {
  core: KeelCore;
  transport: WasmMainTransport;
  log: ReturnType<typeof captureLog>;
  globals(): ReturnType<typeof stubGlobals>;
}

async function boot(
  options: {
    stub?: Parameters<typeof compileStub>[0];
    adapters?: NonNullable<Parameters<typeof KeelCore.attach>[1]["adapters"]>;
    ports?: Readonly<Record<number, PortImpl>>;
    transport?: Partial<ConstructorParameters<typeof WasmMainTransport>[0]>;
    onClose?: (error: Error) => void;
  } = {},
): Promise<Boot> {
  const wasm = await compileStub(options.stub);
  const log = captureLog();
  const transport = new WasmMainTransport({
    wasm,
    expectedSchemaHash: SCHEMA,
    platform: "test",
    onError: (error) => {
      log.log(4, "test", `import failed: ${error instanceof Error ? error.message : String(error)}`);
    },
    ...(options.adapters?.clock && { clock: options.adapters.clock }),
    ...(options.adapters?.rng && { rng: options.adapters.rng }),
    ...(options.adapters?.timer && { timer: options.adapters.timer }),
    ...options.transport,
  });
  const core = track(
    await KeelCore.attach(transport, {
      expectedSchemaHash: SCHEMA,
      shared: false,
      adapters: { log, ...options.adapters },
      ...(options.ports && { ports: options.ports }),
      ...(options.onClose && { onClose: options.onClose }),
    }),
  );
  return { core, transport, log, globals: () => stubGlobals(transport.instance as WebAssembly.Instance) };
}

describe("WasmMainTransport startup", () => {
  it("runs _initialize, checks the ABI and the schema, and configures the core", async () => {
    const { core, transport, globals } = await boot();
    expect(globals().initialized).toBe(1);
    expect(core.hello.schemaHash).toBe(SCHEMA);
    expect(core.hello.mode).toBe("inproc");
    expect(core.mode).toBe("wasm-main");
    // keel_init received an encoded RuntimeConfig: platform, mode, core_threads, blocking_threads, log_level.
    const memory = (transport.instance as WebAssembly.Instance).exports.memory as WebAssembly.Memory;
    const cfg = new KeelReader(new Uint8Array(memory.buffer, 0x400, globals().init_len));
    expect([cfg.readStr(), cfg.readStr(), cfg.readU8(), cfg.readU8(), cfg.readU8()]).toEqual(["test", "inproc", 0, 0, 2]);
    cfg.finish();
  });

  it("reports devtools as mode dev and forwards the log level", async () => {
    const { core, transport, globals } = await boot({ transport: { devtools: true, logLevel: 4 } });
    expect(core.hello.mode).toBe("dev");
    const memory = (transport.instance as WebAssembly.Instance).exports.memory as WebAssembly.Memory;
    const cfg = new KeelReader(new Uint8Array(memory.buffer, 0x400, globals().init_len));
    cfg.readStr();
    expect(cfg.readStr()).toBe("dev");
    cfg.readU8();
    cfg.readU8();
    expect(cfg.readU8()).toBe(4);
  });

  it("refuses a core built from another schema before keel_init runs", async () => {
    const wasm = await compileStub({ schemaHash: 0xfeed_face_cafe_beefn });
    const transport = new WasmMainTransport({ wasm, expectedSchemaHash: SCHEMA });
    const failure = await KeelCore.attach(transport, { expectedSchemaHash: SCHEMA, shared: false }).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(KeelSchemaMismatchError);
    expect(failure).toMatchObject({ expected: SCHEMA, got: 0xfeed_face_cafe_beefn });
    expect((failure as Error).message).toContain("0xfeedfacecafebeef");
    expect(stubGlobals(transport.instance as WebAssembly.Instance).init_len).toBe(-1);
  });

  it("refuses an unknown ABI version", async () => {
    const wasm = await compileStub({ abiVersion: 2 });
    const failure = await KeelCore.attach(new WasmMainTransport({ wasm, expectedSchemaHash: SCHEMA }), {
      expectedSchemaHash: SCHEMA,
      shared: false,
    }).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(KeelTransportError);
    expect(failure).toMatchObject({ reason: "handshake" });
    expect((failure as Error).message).toContain("ABI 2");
  });

  it("reports a failing keel_init", async () => {
    const wasm = await compileStub({ initResult: 7 });
    const failure = await KeelCore.attach(new WasmMainTransport({ wasm, expectedSchemaHash: SCHEMA }), {
      expectedSchemaHash: SCHEMA,
      shared: false,
    }).catch((e: unknown) => e);
    expect(failure).toMatchObject({ reason: "handshake" });
    expect((failure as Error).message).toContain("code 7");
  });

  it("names the exports a module lacks", async () => {
    const wasm = await assemble('(module (memory (export "memory") 1) (func (export "keel_alloc") (param i32) (result i32) (i32.const 0)))');
    const failure = await new WasmMainTransport({ wasm, expectedSchemaHash: SCHEMA })
      .start({
        reply() {},
        changeSet() {},
        streamItem() {},
        portCall: () => ({ kind: "unavailable" }),
        log() {},
        closed() {},
      })
      .catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(KeelTransportError);
    expect((failure as Error).message).toContain("keel_call");
    expect((failure as Error).message).not.toContain("keel_alloc,");
  });

  it("rejects bytes that are not wasm", async () => {
    const failure = await KeelCore.attach(new WasmMainTransport({ wasm: bytesOf(1, 2, 3), expectedSchemaHash: SCHEMA }), {
      expectedSchemaHash: SCHEMA,
      shared: false,
    }).catch((e: unknown) => e);
    expect(failure).toMatchObject({ reason: "handshake" });
    expect((failure as Error).message).toContain("could not instantiate");
  });

  it("accepts a compiled WebAssembly.Module", async () => {
    const module = new WebAssembly.Module((await compileStub()) as BufferSource);
    const core = track(
      await KeelCore.attach(new WasmMainTransport({ wasm: module, expectedSchemaHash: SCHEMA }), {
        expectedSchemaHash: SCHEMA,
        shared: false,
      }),
    );
    expect(core.hello.schemaHash).toBe(SCHEMA);
  });

  it("fetches a URL, streaming when the server says application/wasm", async () => {
    const wasm = await compileStub();
    const fetchMock = vi.fn(async () => new Response(wasm as BodyInit, { headers: { "content-type": "application/wasm" } }));
    vi.stubGlobal("fetch", fetchMock);
    try {
      const core = track(
        await KeelCore.attach(new WasmMainTransport({ wasm: new URL("https://example.test/core.wasm"), expectedSchemaHash: SCHEMA }), {
          expectedSchemaHash: SCHEMA,
          shared: false,
        }),
      );
      expect(fetchMock).toHaveBeenCalledOnce();
      expect(core.hello.schemaHash).toBe(SCHEMA);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("falls back to arrayBuffer for a wrong content type and reports HTTP failures", async () => {
    const wasm = await compileStub();
    vi.stubGlobal("fetch", async () => new Response(wasm as BodyInit, { headers: { "content-type": "application/octet-stream" } }));
    try {
      const core = track(
        await KeelCore.attach(new WasmMainTransport({ wasm: new URL("https://example.test/core.wasm"), expectedSchemaHash: SCHEMA }), {
          expectedSchemaHash: SCHEMA,
          shared: false,
        }),
      );
      expect(core.hello.schemaHash).toBe(SCHEMA);
      vi.stubGlobal("fetch", async () => new Response("nope", { status: 404 }));
      const failure = await new WasmMainTransport({ wasm: new URL("https://example.test/missing.wasm"), expectedSchemaHash: SCHEMA })
        .start({ reply() {}, changeSet() {}, streamItem() {}, portCall: () => ({ kind: "unavailable" }), log() {}, closed() {} })
        .catch((e: unknown) => e);
      expect((failure as Error).message).toContain("404");
    } finally {
      vi.unstubAllGlobals();
    }
  });
});

describe("calls into the core", () => {
  it("call: a synchronous method replies before the promise is awaited", async () => {
    const { core } = await boot();
    await expect(core.call(FREE, STUB.ECHO, utf8("ping"))).resolves.toEqual(utf8("ping"));
  });

  it("call: replies are routed by call id when they interleave", async () => {
    const { core } = await boot();
    const [a, b, c] = await Promise.all([
      core.call(FREE, STUB.ECHO, bytesOf(1)),
      core.call(FREE, STUB.ECHO, bytesOf(2, 2)),
      core.call(FREE, STUB.ECHO, bytesOf(3, 3, 3)),
    ]);
    expect([a, b, c]).toEqual([bytesOf(1), bytesOf(2, 2), bytesOf(3, 3, 3)]);
  });

  it("call: an unknown method is a bad request with the core's reason", async () => {
    const { core } = await boot();
    const failure = (await core.call(FREE, 999, new Uint8Array(0)).catch((e: unknown) => e)) as KeelReplyError;
    expect(failure).toBeInstanceOf(KeelReplyError);
    expect(failure.status).toBe(ReplyStatus.BadRequest);
    expect(failure.reason).toBe("unknown method");
  });

  it("call: object methods carry their handle", async () => {
    const { core } = await boot();
    await expect(core.call({ target: CallTarget.ObjectMethod, handle: 0x0000_0002_0000_0009n }, STUB.ECHO, bytesOf(9))).resolves.toEqual(bytesOf(9));
  });

  it("call: a payload larger than the scratch buffer and than wasm memory grows memory and survives", async () => {
    const { core, globals } = await boot();
    const big = new Uint8Array(300_000).map((_, i) => i % 251);
    const before = globals().alloc_count;
    await expect(core.call(FREE, STUB.ECHO, big)).resolves.toEqual(big);
    expect(globals().alloc_count).toBeGreaterThan(before);
    // Small payloads afterwards still work through the reused scratch buffer.
    await expect(core.call(FREE, STUB.ECHO, bytesOf(5))).resolves.toEqual(bytesOf(5));
  });

  it("call: reuses one scratch buffer for small payloads instead of allocating per call", async () => {
    const { core, globals } = await boot();
    await core.call(FREE, STUB.ECHO, bytesOf(1));
    const allocs = globals().alloc_count;
    const frees = globals().free_count;
    for (let i = 0; i < 20; i++) await core.call(FREE, STUB.ECHO, bytesOf(i));
    // Only the stub's own reply allocations (one per call); nothing is allocated or freed by the host.
    expect(globals().alloc_count - allocs).toBe(20);
    expect(globals().free_count).toBe(frees);
  });

  it("callSync: returns the reply body, frees the KeelBuf, and rejects non-sync methods", async () => {
    const { core, globals } = await boot();
    expect(core.callSync(FREE, STUB.ECHO, utf8("sync!"))).toEqual(utf8("sync!"));
    expect(globals().buf_free_count).toBe(1);
    expect(() => core.callSync(FREE, STUB.ECHO_ASYNC, new Uint8Array(0))).toThrow(KeelReplyError);
    try {
      core.callSync(FREE, 999, new Uint8Array(0));
    } catch (error) {
      expect((error as KeelReplyError).status).toBe(ReplyStatus.BadRequest);
    }
    expect(globals().buf_free_count).toBe(3);
  });

  it("an asynchronous method replies from keel_poll, scheduled on a microtask", async () => {
    const { core, globals } = await boot();
    const reply = core.call(FREE, STUB.ECHO_ASYNC, bytesOf(4, 5));
    expect(globals().poll_count).toBe(0);
    await expect(reply).resolves.toEqual(bytesOf(4, 5));
    expect(globals().poll_count).toBe(1);
  });

  it("construct: returns the handle as a bigint and counts it", async () => {
    const { core } = await boot();
    await expect(core.construct(0x1234, 0x5678, new Uint8Array(0))).resolves.toBe(STUB.HANDLE);
    expect((await core.stats()).liveHandles).toBe(3); // the core's own count wins when it reports one
  });

  it("a call the core refuses without a reply (call id 0) throws a bad request", async () => {
    const { transport } = await boot();
    const payload = encodeCall({ target: CallTarget.FreeFunction, methodId: STUB.ECHO, callId: 0, args: new Uint8Array(0) });
    try {
      transport.send(Kind.Call, payload);
      expect.unreachable();
    } catch (error) {
      expect(error).toBeInstanceOf(KeelReplyError);
      expect((error as KeelReplyError).status).toBe(ReplyStatus.BadRequest);
      expect((error as KeelReplyError).reason).toContain("refused");
    }
  });
});

describe("host-to-core messages", () => {
  it("cancel, credit, event, release and observe reach the matching exports with unsigned arguments", async () => {
    const { core, transport, globals } = await boot();
    transport.send(Kind.Cancel, encodeCancel({ callId: 0xffff_fff0 }));
    expect(globals().cancel_last >>> 0).toBe(0xffff_fff0);
    transport.send(Kind.StreamCredit, encodeStreamCredit({ callId: 3, credit: 9 }));
    expect(globals().credit_total).toBe(9);
    core.event(0xdead_beef, 0xfeed_f00d, bytesOf(1, 2, 3));
    expect(globals().event_port >>> 0).toBe(0xdead_beef);
    expect(globals().event_method >>> 0).toBe(0xfeed_f00d);
    expect(globals().event_len).toBe(3);
    core.event(1, 2, new Uint8Array(0));
    expect(globals().event_len).toBe(0);
    core.release(0xffff_ffff_0000_0005n);
    expect(globals().release_lo).toBe(5);
    expect(globals().release_hi >>> 0).toBe(0xffff_ffff);
  });

  it("observe applies the initial change-set before the promise resolves", async () => {
    const { core } = await boot();
    const seen: Array<[number, ChangeOp, number]> = [];
    core.mirror.register(STUB.HANDLE, (signalId, op, value) => {
      seen.push([signalId, op, new DataView(value.buffer, value.byteOffset, value.byteLength).getUint32(0, true)]);
    });
    await core.observe(STUB.HANDLE, 3, true);
    expect(seen).toEqual([[3, ChangeOp.FullValue, 42]]);
    seen.length = 0;
    await core.observe(STUB.HANDLE, ALL_SIGNALS, true);
    expect(seen).toEqual([[0, ChangeOp.FullValue, 42]]);
    await core.observe(STUB.HANDLE, 3, false);
    expect(seen).toHaveLength(1);
  });

  it("send rejects kinds that only flow from the core", async () => {
    const { transport } = await boot();
    expect(() => transport.send(Kind.Reply, new Uint8Array(5))).toThrow(KeelTransportError);
    expect(() => transport.send(Kind.Hello, encodeHello({ keelVersion: "x", schemaHash: 0n, platform: "p", mode: "m" }))).toThrow(/Hello/);
    expect(() => transport.send(Kind.Restore, new Uint8Array(4))).toThrow(/keel_restore/);
  });
});

describe("imports", () => {
  it("port_call with a synchronous port replies through keel_port_reply before returning 0", async () => {
    const seen: Uint8Array[] = [];
    const impl: PortImpl = {
      sync: true,
      methods: {
        [STUB.PORT_METHOD]: (args) => {
          seen.push(args);
          return Uint8Array.from(args).reverse();
        },
      },
    };
    const { core } = await boot({ ports: { [STUB.PORT_ID]: impl } });
    const reply = await core.call(FREE, STUB.PORT, bytesOf(1, 2, 3));
    expect(seen).toEqual([bytesOf(1, 2, 3)]);
    expect(decodePortReply(reply)).toEqual({ portCallId: 1, status: PortStatus.Ok, body: bytesOf(3, 2, 1) });
  });

  it("port_call with an asynchronous port answers with a later PortReply", async () => {
    const gate = deferred();
    const impl: PortImpl = {
      sync: false,
      methods: {
        [STUB.PORT_METHOD]: async (args) => {
          await gate.promise;
          return Uint8Array.from(args).map((b) => b + 1);
        },
      },
    };
    const { core } = await boot({ ports: { [STUB.PORT_ID]: impl } });
    let settled = false;
    const reply = core.call(FREE, STUB.PORT, bytesOf(1, 2)).then((body) => {
      settled = true;
      return body;
    });
    await macrotask();
    expect(settled).toBe(false);
    gate.resolve();
    expect(decodePortReply(await reply)).toEqual({ portCallId: 1, status: PortStatus.Ok, body: bytesOf(2, 3) });
  });

  it("port_call: a KeelPortError becomes status 1 with its body; another exception is logged and unavailable", async () => {
    const impl: PortImpl = {
      sync: true,
      methods: {
        [STUB.PORT_METHOD]: (args) => {
          if (args[0] === 1) throw new KeelPortError(bytesOf(7, 7));
          throw new Error("boom");
        },
      },
    };
    const { core, log } = await boot({ ports: { [STUB.PORT_ID]: impl } });
    expect(decodePortReply(await core.call(FREE, STUB.PORT, bytesOf(1)))).toEqual({
      portCallId: 1,
      status: PortStatus.Error,
      body: bytesOf(7, 7),
    });
    expect(decodePortReply(await core.call(FREE, STUB.PORT, bytesOf(2)))).toMatchObject({ status: PortStatus.Unavailable });
    expect(log.records.some((r) => r.level === 4 && r.message.includes("boom"))).toBe(true);
  });

  it("port_call: an unregistered port or method is unavailable", async () => {
    const { core } = await boot();
    const failure = (await core.call(FREE, STUB.PORT, new Uint8Array(0)).catch((e: unknown) => e)) as KeelReplyError;
    expect(failure.reason).toBe("port unavailable");
    core.registerPort(STUB.PORT_ID, { sync: true, methods: {} });
    expect(((await core.call(FREE, STUB.PORT, new Uint8Array(0)).catch((e: unknown) => e)) as KeelReplyError).reason).toBe("port unavailable");
  });

  it("a sync port that returns a promise anyway is answered asynchronously", async () => {
    const impl: PortImpl = { sync: true, methods: { [STUB.PORT_METHOD]: (() => Promise.resolve(bytesOf(9))) as unknown as (a: Uint8Array) => Uint8Array } };
    const { core } = await boot({ ports: { [STUB.PORT_ID]: impl } });
    expect(decodePortReply(await core.call(FREE, STUB.PORT, new Uint8Array(0))).body).toEqual(bytesOf(9));
  });

  it("timer_set arms the timer adapter with an unsigned id, and keel_timer_fired completes the call", async () => {
    const armed: Array<{ id: number; delay: number }> = [];
    let fire: ((id: number) => void) | undefined;
    const { core } = await boot({
      adapters: {
        timer: {
          set(id, delay, f) {
            armed.push({ id, delay });
            fire = f;
          },
        },
      },
    });
    const done = core.call(FREE, STUB.TIMER, new Uint8Array(0));
    expect(armed).toEqual([{ id: STUB.TIMER_ID, delay: 50 }]);
    (fire as (id: number) => void)(STUB.TIMER_ID);
    const body = await done;
    expect(new DataView(body.buffer, body.byteOffset, 4).getUint32(0, true)).toBe(STUB.TIMER_ID);
  });

  it("the default timer uses setTimeout", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout"] });
    try {
      const { core } = await boot({ adapters: { timer: null } });
      const done = core.call(FREE, STUB.TIMER, new Uint8Array(0));
      await vi.advanceTimersByTimeAsync(49);
      let settled = false;
      void done.then(() => {
        settled = true;
      });
      await microtasks();
      expect(settled).toBe(false);
      await vi.advanceTimersByTimeAsync(1);
      await expect(done).resolves.toHaveLength(4);
    } finally {
      vi.useRealTimers();
    }
  });

  it("log: structured (target + message) and plain text records reach the log adapter", async () => {
    const { core, log } = await boot();
    const w = new KeelWriter();
    w.writeStr("my::target");
    w.writeStr("structured message");
    await core.call(FREE, STUB.LOG, w.finish());
    await core.call(FREE, STUB.LOG, utf8("plain text"));
    expect(log.records).toEqual([
      { level: 3, target: "my::target", message: "structured message" },
      { level: 3, target: "keel", message: "plain text" },
    ]);
  });

  it("now_ms and random use the clock and rng adapters", async () => {
    const { core } = await boot({
      adapters: {
        clock: { nowMs: () => 1234.5, monotonicNs: () => 0n },
        rng: {
          fill(out) {
            out.forEach((_, i) => {
              out[i] = i + 1;
            });
          },
        },
      },
    });
    const body = await core.call(FREE, STUB.RANDOM_NOW, new Uint8Array(0));
    expect([...body.subarray(0, 8)]).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
    expect(new DataView(body.buffer, body.byteOffset + 8, 8).getFloat64(0, true)).toBe(1234.5);
  });

  it("now_ms and random default to Date.now and WebCrypto", async () => {
    const { core } = await boot();
    const before = Date.now();
    const body = await core.call(FREE, STUB.RANDOM_NOW, new Uint8Array(0));
    const now = new DataView(body.buffer, body.byteOffset + 8, 8).getFloat64(0, true);
    expect(now).toBeGreaterThanOrEqual(before);
    expect(now).toBeLessThanOrEqual(Date.now());
    expect(body.subarray(0, 8).some((b) => b !== 0)).toBe(true);
  });

  it("a stream call delivers its item and end, and grants the initial credit", async () => {
    const { core, globals } = await boot();
    const items: Uint8Array[] = [];
    for await (const item of core.stream(FREE, STUB.STREAM, utf8("only"))) items.push(item);
    expect(items).toEqual([utf8("only")]);
    expect(globals().credit_total).toBe(16);
  });
});

describe("failure", () => {
  it("a trap kills the transport: the call fails, the log record survives, later calls fail the same way, onClose runs once", async () => {
    const onClose = vi.fn();
    const { core, log } = await boot({ onClose });
    const other = core.call(FREE, STUB.ECHO_ASYNC, bytesOf(1)); // pending when the trap hits
    const failure = (await core.call(FREE, STUB.PANIC, new Uint8Array(0)).catch((e: unknown) => e)) as KeelTransportError;
    expect(failure).toBeInstanceOf(KeelTransportError);
    expect(failure.reason).toBe("trap");
    expect(log.records).toContainEqual({ level: 5, target: "keel", message: "unknown method" });
    await expect(other).rejects.toMatchObject({ reason: "trap" });
    await microtasks();
    expect(core.closed).toBe(true);
    expect(onClose).toHaveBeenCalledOnce();
    expect(onClose.mock.calls[0]?.[0]).toBe(failure);
    await expect(core.call(FREE, STUB.ECHO, new Uint8Array(0))).rejects.toBeInstanceOf(KeelTransportError);
  });

  it("a keel_alloc that returns 0 fails the transport instead of writing at linear address 0 (review L3)", async () => {
    const onClose = vi.fn();
    const { core, transport } = await boot({ onClose });
    const exports = (transport.instance as WebAssembly.Instance).exports;
    (exports.alloc_zero as WebAssembly.Global).value = 1;
    // Only a payload above the scratch buffer's limit is allocated per call.
    const big = new Uint8Array(100_000).fill(0xaa);
    expect(() => core.callSync(FREE, STUB.ECHO, big)).toThrow(/keel_alloc\(\d+\) returned 0 instead of trapping/);
    await microtasks();
    expect(core.closed).toBe(true);
    expect(onClose).toHaveBeenCalledOnce();
    // Nothing was copied to linear address 0 (the bottom of a real module's shadow stack).
    expect(new Uint8Array((exports.memory as WebAssembly.Memory).buffer, 0, 64).every((b) => b === 0)).toBe(true);
  });

  it("close makes every later use fail and ignores the timers and polls still in flight", async () => {
    const { core, transport } = await boot();
    const pending = core.call(FREE, STUB.ECHO_ASYNC, bytesOf(1));
    core.close();
    await expect(pending).rejects.toMatchObject({ reason: "closed" });
    expect(() => core.callSync(FREE, STUB.ECHO, new Uint8Array(0))).toThrow(/closed/);
    expect(() => transport.send(Kind.Cancel, encodeCancel({ callId: 1 }))).toThrow(KeelTransportError);
    await microtasks();
  });

  it("an import handler that throws is contained: the core keeps running and the failure is reported", async () => {
    const { core, log } = await boot({
      adapters: {
        rng: {
          fill() {
            throw new Error("entropy exhausted");
          },
        },
      },
    });
    await expect(core.call(FREE, STUB.RANDOM_NOW, new Uint8Array(0))).resolves.toHaveLength(16);
    expect(log.records.some((r) => r.message.includes("entropy exhausted"))).toBe(true);
    await expect(core.call(FREE, STUB.ECHO, bytesOf(1))).resolves.toEqual(bytesOf(1));
  });

  it("callSync is a mode error over a transport without callSync", async () => {
    const asyncOnly = {
      mode: "remote",
      synchronous: false,
      start: () => Promise.resolve({ keelVersion: "x", schemaHash: SCHEMA, platform: "p", mode: "m" }),
      send: () => {},
      close: () => {},
    };
    const core = track(await KeelCore.attach(asyncOnly, { expectedSchemaHash: SCHEMA, shared: false }));
    expect(() => core.callSync(FREE, 1, new Uint8Array(0))).toThrow(KeelModeError);
    expect(() => core.callSync(FREE, 1, new Uint8Array(0))).toThrow("callSync is not available in mode 'remote'");
  });
});

describe("statistics", () => {
  it("reports the core's own numbers next to the host's", async () => {
    const { core } = await boot();
    core.mirror.register(5n, () => {});
    const stats = await core.stats();
    expect(stats.core).toMatchObject({ live_handles: 3, platform: "stub" });
    expect(stats.mirroredStores).toBe(1);
    expect(stats.pendingCalls).toBe(0);
    expect(stats.openStreams).toBe(0);
  });
});
