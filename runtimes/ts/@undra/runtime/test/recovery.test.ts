import { afterEach, describe, expect, it, vi } from "vitest";
import { UndraCallError, UndraUnhandledError } from "../src/call-error.js";
import { UndraCore } from "../src/core.js";
import { UndraTransportError } from "../src/errors.js";
import { UndraStore } from "../src/object.js";
import type { PortImpl } from "../src/port.js";
import {
  DEFAULT_RECOVERY,
  type RestartResult,
  SnapshotKeeper,
  UndraCoreRestarted,
  emptySnapshot,
  crashRecovery,
  resolveRecovery,
  snapshotStoreHandles,
  withGenerationFloor,
} from "../src/recovery.js";
import { Signal } from "../src/signal.js";
import { type UndraPanicReport, panicReport } from "../src/panic.js";
import { WasmMainTransport } from "../src/transport/wasm-main.js";
import { WasmWorkerTransport } from "../src/transport/wasm-worker.js";
import {
  ALL_SIGNALS,
  CallTarget,
  ChangeOp,
  Kind,
  codecs,
  decodeSnapshot,
  decodeValue,
  encodeSnapshot,
  encodeValue,
  makeHandle,
} from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { captureLog, deferred, macrotask, microtasks, track } from "./support/harness.js";
import { CounterStore, u32 } from "./support/store.js";
import { STUB, compileStub } from "./support/stub-core.js";
import { channelWorker } from "./support/worker.js";

/*
 * Recovering a web core that trapped (ADR-049 decision 3): the options, the snapshot keeper, the panic report
 * (ADR-046 decision 4.4, minimal), the restart sequence of UndraCore over a scripted transport (every step and its
 * order, the budget, a restart that traps again), and the two wasm transports over the stub core. The real core in
 * both modes is covered by crates/undra-ffi/tests/wasm/ts-runtime.test.mjs.
 */

const FREE = { target: CallTarget.FreeFunction } as const;
const QUERY_TYPE = 0x5151_0001;
const QUERY_NEW = 0x5151_0001;
const OBJECT_TYPE = 0x0b1e_0001;
const OBJECT_NEW = 0x0b1e_0002;
const PENDING = 0x7000_0001;
const STREAM = 0x7000_0002;
const ECHO = 0x7000_0003;

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

/** A query handle as `undra-bindgen` writes one (ADR-049): it records its constructor call. */
class QueryStore extends UndraStore {
  readonly data = new Signal<number>(0);

  private constructor(core: UndraCore, handle: bigint, args: Uint8Array) {
    super(core, handle, { recreate: { typeId: QUERY_TYPE, methodId: QUERY_NEW, args } });
    this._signals = [this.data];
  }

  static async create(core: UndraCore, key: number): Promise<QueryStore> {
    const args = u32(key);
    const handle = await core.construct(QUERY_TYPE, QUERY_NEW, args);
    const store = new QueryStore(core, handle, args);
    await store._observeAll();
    return store;
  }

  protected override _apply(signalId: number, _op: ChangeOp, value: Uint8Array): void {
    if (signalId === 0) this.data._set(decodeValue(codecs.u32, value));
  }
}

/** A trap as the wasm transports report one: an `UndraTransportError("trap")` whose cause is the engine's error. */
function trapError(text = "unreachable"): UndraTransportError {
  const cause = new Error(text);
  cause.name = "RuntimeError";
  cause.stack = `RuntimeError: ${text}\n    at undra_core.wasm.core::panicking::panic (wasm://wasm/0012abcd:wasm-function[123]:0x4567)\n    at undra_call (wasm://wasm/0012abcd:wasm-function[9]:0x89)\n    at WasmMainTransport.send (file:///runtime.js:1:1)`;
  return new UndraTransportError("trap", `the wasm core trapped: ${text}`, { cause });
}

/** A scripted transport that can restart: each restart takes the next scripted outcome, else succeeds with the stores in `restoredStores`. */
class RestartableFake extends FakeCoreTransport {
  readonly floors: number[] = [];
  readonly outcomes: Array<() => Promise<RestartResult>> = [];
  restoredStores: bigint[] = [];

  constructor() {
    super({ mode: "wasm-worker" });
  }

  restart(floor: number): Promise<RestartResult> {
    this.floors.push(floor);
    const next = this.outcomes.shift();
    if (next !== undefined) return next();
    return Promise.resolve({
      hello: { undraVersion: "fake-2", schemaHash: this.schemaHash, platform: "fake", mode: "test" },
      restoredFromAgeMs: 250,
      storeHandles: [...this.restoredStores],
    });
  }

  /** The core traps (after logging its panic, as a wasm core does). */
  trap(message = "kaboom"): void {
    this.emitLog(5, "undra::panic", message);
    this.fail(trapError());
  }
}

async function recovering(options: { recovery?: false | Parameters<typeof crashRecovery>[0]; ports?: Record<number, PortImpl> } = {}) {
  const fake = new RestartableFake();
  const log = captureLog();
  const panics: UndraPanicReport[] = [];
  const restarts: UndraCoreRestarted[] = [];
  const errors: UndraUnhandledError[] = [];
  const closed: Error[] = [];
  let nextHandle = 1;
  const issue = (generation: number): bigint => makeHandle(nextHandle++, generation);
  fake.on(QUERY_NEW, (call, r) => {
    const handle = issue(4);
    fake.store(handle, new Map([[0, u32(decodeValue(codecs.u32, "args" in call ? call.args : new Uint8Array(0)) * 10)]]));
    r.ok(encodeValue(codecs.u64, handle));
  });
  fake.on(OBJECT_NEW, (_call, r) => {
    r.ok(encodeValue(codecs.u64, issue(7)));
  });
  fake.on(PENDING, (_call, r) => {
    r.defer();
  });
  fake.echo(ECHO);
  fake.on(STREAM, (_call, r) => {
    r.defer();
  });
  const core = track(
    await UndraCore.attach(fake, {
      expectedSchemaHash: SCHEMA,
      shared: false,
      adapters: { log, http: null, timer: null, kv: null, secureStore: null, fs: null, connectivity: null, lifecycle: null },
      ...(options.recovery !== false && { recovery: crashRecovery(options.recovery) }),
      onPanic: (report) => panics.push(report),
      onCoreRestarted: (event) => restarts.push(event),
      onError: (error) => errors.push(error),
      onClose: (error) => closed.push(error),
      ...(options.ports && { ports: options.ports }),
    }),
  );
  return { fake, core, log, panics, restarts, errors, closed, issue };
}

const until = async (what: string, probe: () => boolean): Promise<void> => {
  for (let i = 0; i < 200; i++) {
    if (probe()) return;
    await macrotask();
  }
  throw new Error(`timed out waiting for ${what}`);
};

describe("crashRecovery (LoadOptions.recovery)", () => {
  it("takes the defaults of ADR-049; an object overrides some", () => {
    expect(resolveRecovery()).toEqual({ snapshotEveryMs: 1000, maxSnapshotBytes: 4 * 1024 * 1024, maxRestarts: 3, perMs: 60_000 });
    expect(DEFAULT_RECOVERY).toEqual(resolveRecovery());
    expect(crashRecovery().options).toEqual(DEFAULT_RECOVERY);
    expect(crashRecovery({ maxRestarts: 1 }).options).toEqual({ ...DEFAULT_RECOVERY, maxRestarts: 1 });
    expect(resolveRecovery({ maxRestarts: 1, snapshotEveryMs: 10 })).toEqual({ ...DEFAULT_RECOVERY, maxRestarts: 1, snapshotEveryMs: 10 });
    expect(resolveRecovery({ perMs: Number.NaN, maxRestarts: -2 })).toEqual({ ...DEFAULT_RECOVERY, maxRestarts: 0 });
  });

  it("belongs to one core: a second core cannot use the same one", async () => {
    const recovery = crashRecovery();
    await UndraCore.attach(new FakeCoreTransport(), { expectedSchemaHash: SCHEMA, shared: false, adapters: { log: captureLog() }, recovery }).then(track);
    const second = await UndraCore.attach(new FakeCoreTransport(), { expectedSchemaHash: SCHEMA, shared: false, adapters: { log: captureLog() }, recovery }).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(second).toBeInstanceOf(UndraTransportError);
    expect((second as UndraTransportError).message).toMatch(/one per core/);
  });
});

describe("SnapshotKeeper", () => {
  const source = (sizes: number[] = []) => {
    let n = 0;
    const events: string[] = [];
    return {
      events,
      take: () => {
        const size = sizes[n] ?? 8;
        n++;
        events.push(`take ${size}`);
        return new Uint8Array(size).fill(n);
      },
      tooLarge: (bytes: number, limit: number) => events.push(`too large ${bytes} > ${limit}`),
      failed: (error: unknown) => events.push(`failed ${String(error)}`),
    };
  };

  it("snapshots once after a burst of changes, at most once per snapshotEveryMs, and keeps a copy with its time", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000_000);
    const s = source();
    const keeper = new SnapshotKeeper({ snapshotEveryMs: 1000, maxSnapshotBytes: 100 }, s);
    expect(keeper.last).toBeNull();
    keeper.changed();
    keeper.changed();
    keeper.changed();
    await vi.advanceTimersByTimeAsync(0);
    expect(s.events).toEqual(["take 8"]);
    expect(keeper.last?.takenAt).toBe(1_000_000);
    expect(new Uint8Array(keeper.last?.data as ArrayBuffer)).toEqual(new Uint8Array(8).fill(1));
    // A change right after: not before a second has passed.
    keeper.changed();
    await vi.advanceTimersByTimeAsync(999);
    expect(s.events).toEqual(["take 8"]);
    await vi.advanceTimersByTimeAsync(1);
    expect(s.events).toEqual(["take 8", "take 8"]);
    // No change, no snapshot.
    await vi.advanceTimersByTimeAsync(5000);
    expect(s.events).toHaveLength(2);
  });

  it("does not keep a snapshot over maxSnapshotBytes (the previous one stays) and says so once", async () => {
    vi.useFakeTimers();
    const s = source([8, 200, 300, 9]);
    const keeper = new SnapshotKeeper({ snapshotEveryMs: 0, maxSnapshotBytes: 100 }, s);
    for (let i = 0; i < 4; i++) {
      keeper.changed();
      await vi.advanceTimersByTimeAsync(0);
    }
    expect(s.events).toEqual(["take 8", "take 200", "too large 200 > 100", "take 300", "take 9"]);
    expect(keeper.last?.data.byteLength).toBe(9);
  });

  it("waits for an idle callback where the platform has requestIdleCallback", async () => {
    vi.useFakeTimers();
    const idle: Array<() => void> = [];
    vi.stubGlobal("requestIdleCallback", (fn: () => void) => idle.push(fn));
    try {
      const s = source();
      const keeper = new SnapshotKeeper({ snapshotEveryMs: 0, maxSnapshotBytes: 100 }, s);
      keeper.changed();
      await vi.advanceTimersByTimeAsync(0);
      expect(s.events).toEqual([]);
      idle.shift()?.();
      expect(s.events).toEqual(["take 8"]);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("reports a snapshot that fails, pauses while the core restarts, and stops for good", async () => {
    vi.useFakeTimers();
    const events: string[] = [];
    const keeper = new SnapshotKeeper(
      { snapshotEveryMs: 0, maxSnapshotBytes: 100 },
      {
        take: () => {
          throw new Error("trapped");
        },
        tooLarge: () => {},
        failed: (error) => events.push(String(error)),
      },
    );
    keeper.changed();
    await vi.advanceTimersByTimeAsync(0);
    expect(events).toEqual(["Error: trapped"]);
    keeper.pause();
    keeper.changed();
    await vi.advanceTimersByTimeAsync(10);
    expect(events).toHaveLength(1);
    keeper.resume();
    await vi.advanceTimersByTimeAsync(0);
    expect(events).toHaveLength(2);
    keeper.stop();
    keeper.changed();
    await vi.advanceTimersByTimeAsync(10);
    expect(events).toHaveLength(2);
  });
});

describe("snapshot helpers", () => {
  const snapshot = encodeSnapshot({
    generationFloor: 3,
    schemaHash: 9n,
    types: [{ typeId: 1, fingerprint: 2n }],
    description: "{}",
    stores: [{ handle: makeHandle(4, 2), typeId: 1, signals: [{ signalId: 0, value: u32(5) }] }],
  });

  it("raise the generation floor in a copy, never lower it", () => {
    expect(decodeSnapshot(withGenerationFloor(snapshot, 9)).generationFloor).toBe(9);
    expect(decodeSnapshot(withGenerationFloor(snapshot, 1)).generationFloor).toBe(3);
    expect(decodeSnapshot(snapshot).generationFloor, "the original is untouched").toBe(3);
  });

  it("read the store handles, and make an empty snapshot that only raises the floor", () => {
    expect(snapshotStoreHandles(snapshot)).toEqual([makeHandle(4, 2)]);
    expect(snapshotStoreHandles(Uint8Array.of(1, 2, 3))).toBeNull();
    expect(decodeSnapshot(emptySnapshot(7n, 12))).toEqual({ generationFloor: 12, schemaHash: 7n, types: [], description: "", stores: [] });
  });
});

describe("the panic report (ADR-046 decision 4.4, minimal)", () => {
  it("takes the message of the core's FATAL record and the wasm frames of the trap's stack", () => {
    const report = panicReport("kaboom", trapError(), 0xabcn, "wasm-main");
    expect(report.message).toBe("kaboom");
    expect(report.location).toBe("");
    expect(report.schemaHash).toBe(0xabcn);
    expect(report.trap).toBe("RuntimeError: unreachable");
    expect(report.operation).toBe("wasm-main: RuntimeError: unreachable");
    expect(report.frames).toEqual([
      "undra_core.wasm.core::panicking::panic (wasm://wasm/0012abcd:wasm-function[123]:0x4567)",
      "undra_call (wasm://wasm/0012abcd:wasm-function[9]:0x89)",
    ]);
  });

  it("splits the location off a record that carries it, and falls back to the trap's text without a record", () => {
    expect(panicReport("index out of bounds at src/lib.rs:12", trapError(), 0n, "wasm-worker")).toMatchObject({
      message: "index out of bounds",
      location: "src/lib.rs:12",
    });
    expect(panicReport(null, trapError("stack overflow"), 0n, "wasm-main").message).toBe("RuntimeError: stack overflow");
  });
});

describe("the restart sequence of UndraCore (ADR-049 decision 3.4)", () => {
  it("panic report, then in-flight calls and streams fail 'restarted', restart, re-observe, re-create the query handles, then onCoreRestarted and onError", async () => {
    const t = await recovering();
    const { fake, core } = t;
    // A store the snapshot will bring back, a query handle, and an object that is not a store.
    const counterHandle = makeHandle(50, 3);
    fake.store(counterHandle, new Map([[0, u32(1)]]));
    const counter = await CounterStore.create(core, counterHandle);
    const query = await QueryStore.create(core, 4);
    expect(query.data.peek()).toBe(40);
    const queryHandle = query.handle;
    const object = await core.construct(OBJECT_TYPE, OBJECT_NEW, new Uint8Array(0));
    fake.restoredStores = [counterHandle];

    const pending = core.call(FREE, PENDING, new Uint8Array(0));
    // A stream the core has not answered yet: its first item waits.
    const ending = core.stream(FREE, STREAM, new Uint8Array(0))[Symbol.asyncIterator]().next();
    const order: string[] = [];
    pending.catch((error: unknown) => order.push(`call ${(error as UndraTransportError).reason}`));
    ending.catch((error: unknown) => order.push(`stream ${(error as UndraTransportError).reason}`));
    fake.setSignal(counterHandle, 0, u32(9)); // a change the restored snapshot does not have
    await until("the change", () => counter.count.peek() === 9);
    fake.store(counterHandle, new Map([[0, u32(7)]])); // what the restored core answers an observe with

    fake.trap("kaboom");
    await until("the restart", () => t.restarts.length === 1);

    // 1. The panic report went to onPanic, before anything else.
    expect(t.panics).toHaveLength(1);
    expect(t.panics[0]?.message).toBe("kaboom");
    // 2. Everything in flight failed with "restarted"; generated code reads it as Unavailable.
    await expect(pending).rejects.toMatchObject({ reason: "restarted" });
    await expect(ending).rejects.toMatchObject({ reason: "restarted" });
    expect(order.sort()).toEqual(["call restarted", "stream restarted"]);
    const mapped = UndraCallError.mapped(await pending.catch((e: unknown) => e));
    expect(mapped).toBeInstanceOf(UndraCallError.Unavailable);
    expect((mapped as UndraCallError.Unavailable).transport.reason).toBe("restarted");
    // 3. The transport restarted with the highest generation the host holds as the floor.
    expect(fake.floors).toEqual([7]);
    // 4. The restored store was observed again and shows the restored value, on the same handle.
    expect(counter.handle).toBe(counterHandle);
    expect(counter.count.peek()).toBe(7);
    // 5. The query handle was re-created with its recorded call and the wrapper moved to the new handle.
    expect(query.handle).not.toBe(queryHandle);
    expect(core.mirror.has(queryHandle)).toBe(false);
    expect(core.mirror.has(query.handle)).toBe(true);
    const ctors = fake.calls.filter((c) => "methodId" in c && c.methodId === QUERY_NEW);
    expect(ctors).toHaveLength(2);
    expect(fake.observed.filter((o) => o.handle === query.handle && o.on)).toHaveLength(1);
    expect(query.data.peek()).toBe(40);
    fake.setSignal(query.handle, 0, u32(41));
    await until("the query's change", () => query.data.peek() === 41);
    // 6. One event, to both hooks: the panic, the snapshot's age, the failed calls and the stale object.
    const event = t.restarts[0] as UndraCoreRestarted;
    expect(event).toBeInstanceOf(UndraCoreRestarted);
    expect(event).toBeInstanceOf(UndraUnhandledError);
    expect(event).toMatchObject({ restoredFromAgeMs: 250, rejectedCalls: 2, staleObjects: 1, operation: "wasm core" });
    expect(event.message).toBe("the wasm core trapped (kaboom) and was restarted from a snapshot 250 ms old");
    expect(event.report).toBe(t.panics[0]);
    expect(event.error).toBeInstanceOf(UndraCallError.Panicked);
    expect(t.errors).toEqual([event]);
    expect(t.closed).toEqual([]);
    expect(core.closed).toBe(false);
    expect(core.hello.undraVersion).toBe("fake-2");
    // The core works; the object that is not a store went stale: it is not observed or re-created.
    await expect(core.call(FREE, ECHO, u32(3))).resolves.toEqual(u32(3));
    expect(fake.observed.filter((o) => o.handle === object)).toEqual([]);
  });

  it("refuses calls while the core restarts, and releases what was released meanwhile once it is back", async () => {
    const t = await recovering();
    const gate = deferred<RestartResult>();
    t.fake.outcomes.push(() => gate.promise);
    const handle = makeHandle(60, 1);
    t.fake.store(handle, new Map([[0, u32(1)]]));
    const store = await CounterStore.create(t.core, handle);
    t.fake.trap();
    await macrotask();
    await expect(t.core.call(FREE, ECHO, u32(1))).rejects.toMatchObject({ reason: "restarted" });
    expect(() => t.core.event(1, 2, new Uint8Array(0))).toThrow(/restarting/);
    store.close();
    expect(t.fake.released).toEqual([]);
    gate.resolve({ hello: { undraVersion: "x", schemaHash: SCHEMA, platform: "p", mode: "m" }, restoredFromAgeMs: 10, storeHandles: [] });
    await until("the restart", () => t.restarts.length === 1);
    expect(t.fake.released).toEqual([handle]);
    await expect(t.core.call(FREE, ECHO, u32(1))).resolves.toEqual(u32(1));
  });

  it("past maxRestarts within perMs the core stays dead and onClose reports the trap; the window slides", async () => {
    const now = vi.spyOn(Date, "now");
    now.mockReturnValue(1_000_000);
    const t = await recovering({ recovery: { maxRestarts: 2, perMs: 60_000 } });
    for (let i = 1; i <= 2; i++) {
      t.fake.trap(`panic ${i}`);
      await until(`restart ${i}`, () => t.restarts.length === i);
    }
    // A minute later the first two no longer count.
    now.mockReturnValue(1_000_000 + 60_001);
    t.fake.trap("panic 3");
    await until("restart 3", () => t.restarts.length === 3);
    now.mockReturnValue(1_000_000 + 60_002);
    t.fake.trap("panic 4");
    await until("restart 4", () => t.restarts.length === 4);
    // The fifth within the window: dead.
    t.fake.trap("panic 5");
    await until("the close", () => t.closed.length === 1);
    expect(t.panics.map((p) => p.message)).toEqual(["panic 1", "panic 2", "panic 3", "panic 4", "panic 5"]);
    expect(t.closed[0]).toMatchObject({ reason: "trap" });
    expect(t.core.closed).toBe(true);
    expect(t.core.connection.peek()).toMatchObject({ kind: "closed", reason: "failed" });
    expect(t.fake.floors).toHaveLength(4);
    await expect(t.core.call(FREE, ECHO, u32(1))).rejects.toMatchObject({ reason: "closed" });
  });

  it("a restart that traps again is another trap: it counts, and a deterministic panic loop ends dead", async () => {
    const t = await recovering({ recovery: { maxRestarts: 3 } });
    for (let i = 0; i < 5; i++) t.fake.outcomes.push(() => Promise.reject(trapError("the restored state panics")));
    t.fake.trap("first");
    await until("the close", () => t.closed.length === 1);
    expect(t.fake.floors).toHaveLength(3);
    expect(t.panics).toHaveLength(4);
    expect(t.restarts).toEqual([]);
    expect(t.core.closed).toBe(true);
  });

  it("a restart that fails otherwise ends the core at once", async () => {
    const t = await recovering();
    t.fake.outcomes.push(() => Promise.reject(new UndraTransportError("handshake", "could not instantiate the wasm core again")));
    t.fake.trap();
    await until("the close", () => t.closed.length === 1);
    expect(t.closed[0]).toMatchObject({ reason: "trap" });
    expect(t.log.records.some((r) => r.message.includes("could not be restarted"))).toBe(true);
  });

  it("without recovery a trap ends the core, as before; onPanic still hears the panic", async () => {
    const t = await recovering({ recovery: false });
    t.fake.trap("kaboom");
    await until("the close", () => t.closed.length === 1);
    expect(t.panics.map((p) => p.message)).toEqual(["kaboom"]);
    expect(t.fake.floors).toEqual([]);
    expect(t.restarts).toEqual([]);
  });

  it("a port reply that settles after the restart is not delivered to the new instance", async () => {
    const answer = deferred<Uint8Array>();
    const t = await recovering({ ports: { 0x1234: { sync: false, methods: { 1: () => answer.promise } } } });
    void t.fake.callPort(0x1234, 1);
    await microtasks();
    t.fake.trap();
    await until("the restart", () => t.restarts.length === 1);
    answer.resolve(u32(1));
    await macrotask();
    expect(t.fake.sent.filter((m) => m.kind === Kind.PortReply)).toEqual([]);
  });

  it("a query handle closed before the restart is not re-created; one whose constructor fails stays stale", async () => {
    const t = await recovering();
    const closed = await QueryStore.create(t.core, 1);
    const failing = await QueryStore.create(t.core, 2);
    closed.close();
    t.fake.on(QUERY_NEW, (_call, r) => r.badRequest("no longer"));
    t.fake.trap();
    await until("the restart", () => t.restarts.length === 1);
    expect(t.restarts[0]?.staleObjects).toBe(1);
    expect(failing.handle).toBe(failing.handle);
    expect(t.errors.some((e) => e.operation.startsWith("re-creating a query handle"))).toBe(true);
  });
});

describe("the wasm transports restart over the stub core", () => {
  it("wasm-main: the same compiled module again, the kept snapshot restored, the core usable", async () => {
    const wasm = await compileStub({ snapshot: true });
    const module = await WebAssembly.compile(wasm as Uint8Array<ArrayBuffer>);
    const instantiate = vi.spyOn(WebAssembly, "instantiate");
    const compile = vi.spyOn(WebAssembly, "compile");
    const restarts: UndraCoreRestarted[] = [];
    const transport = new WasmMainTransport({ wasm: module, expectedSchemaHash: STUB.SCHEMA_HASH });
    const recovery = crashRecovery({ snapshotEveryMs: 0, maxSnapshotBytes: 1024 });
    const core = track(
      await UndraCore.attach(transport, {
        expectedSchemaHash: STUB.SCHEMA_HASH,
        shared: false,
        adapters: { log: captureLog() },
        recovery,
        onCoreRestarted: (event) => restarts.push(event),
      }),
    );
    // A change-set makes a snapshot due: observing delivers one.
    await core.observe(STUB.HANDLE, ALL_SIGNALS, true);
    await until("a snapshot", () => recovery.lastSnapshot !== null);
    expect(new Uint8Array(recovery.lastSnapshot?.data as ArrayBuffer)).toEqual(STUB.SNAPSHOT);
    const pending = core.call(FREE, STUB.ECHO_ASYNC, u32(1));
    const failure = await core.call(FREE, STUB.PANIC, new Uint8Array(0)).catch((e: unknown) => e);
    expect(failure).toMatchObject({ reason: "restarted" });
    await expect(pending).rejects.toMatchObject({ reason: "restarted" });
    await until("the restart", () => restarts.length === 1);
    expect(instantiate.mock.calls.at(-1)?.[0]).toBe(module);
    expect(compile).not.toHaveBeenCalled();
    expect(restarts[0]?.restoredFromAgeMs).toBeGreaterThanOrEqual(0);
    // The stub's canned snapshot is not a layout-2 one: the runtime cannot read its stores, so it counts none stale.
    expect(restarts[0]?.staleObjects).toBe(0);
    await expect(core.call(FREE, STUB.ECHO, u32(5))).resolves.toEqual(u32(5));
    expect(core.callSync(FREE, STUB.ECHO, u32(6))).toEqual(u32(6));
  });

  it("wasm-main without recovery: the trap ends the core", async () => {
    const closed: Error[] = [];
    const core = track(
      await UndraCore.attach(new WasmMainTransport({ wasm: await compileStub(), expectedSchemaHash: STUB.SCHEMA_HASH }), {
        expectedSchemaHash: STUB.SCHEMA_HASH,
        shared: false,
        adapters: { log: captureLog() },
        onClose: (error) => closed.push(error),
      }),
    );
    await expect(core.call(FREE, STUB.PANIC, new Uint8Array(0))).rejects.toMatchObject({ reason: "trap" });
    await until("the close", () => closed.length === 1);
  });

  it.each(["wasm-main", "wasm-worker"] as const)(
    "%s: a port call the new instance makes while it initialises is answered (the cache an init hook reads)",
    async (mode) => {
      const module = await WebAssembly.compile((await compileStub({ snapshot: true, portOnInit: true })) as Uint8Array<ArrayBuffer>);
      let asked = 0;
      const port: PortImpl = {
        sync: false,
        methods: {
          // Answered from a microtask: before the worker's `restarted` message is read on this thread.
          [STUB.PORT_METHOD]: async () => {
            asked++;
            await Promise.resolve();
            return u32(40 + asked);
          },
        },
      };
      const worker = mode === "wasm-worker" ? channelWorker() : null;
      const policy = { snapshotEveryMs: 0, maxSnapshotBytes: 1024 };
      const transport =
        worker === null
          ? new WasmMainTransport({ wasm: module, expectedSchemaHash: STUB.SCHEMA_HASH })
          : new WasmWorkerTransport({ wasm: module, expectedSchemaHash: STUB.SCHEMA_HASH, worker: worker.host, recovery: policy });
      const restarts: UndraCoreRestarted[] = [];
      const core = track(
        await UndraCore.attach(transport, {
          expectedSchemaHash: STUB.SCHEMA_HASH,
          shared: false,
          adapters: { log: captureLog(), http: null, timer: null },
          ports: { [STUB.PORT_ID]: port },
          recovery: crashRecovery(policy),
          onCoreRestarted: (event) => restarts.push(event),
        }),
      );
      await until("the first init's port reply", () => asked === 1);
      await expect(core.call(FREE, STUB.PANIC, new Uint8Array(0))).rejects.toMatchObject({ reason: "restarted" });
      await until("the restart", () => restarts.length === 1);
      await until("the second init's port call", () => asked === 2);
      // What the new instance heard back from the port call its `undra_init` made: PortReply { id, Ok, u32 42 }.
      let heard: Uint8Array = new Uint8Array(0);
      await until("the reply to reach the new instance", () => {
        // The last of these calls may still be in flight when the core closes below: its rejection is expected.
        core.call(FREE, STUB.INIT_PORT_REPLY, new Uint8Array(0)).then(
          (body) => {
            heard = body;
          },
          () => {},
        );
        return heard.length > 0;
      });
      expect([...heard.subarray(4)]).toEqual([0, ...u32(42)]);
      core.close();
      worker?.close();
    },
  );

  it("wasm-worker: the worker keeps the snapshot and restarts the core itself", async () => {
    const module = await WebAssembly.compile((await compileStub({ snapshot: true })) as Uint8Array<ArrayBuffer>);
    const worker = channelWorker();
    const restarts: UndraCoreRestarted[] = [];
    const closed: Error[] = [];
    const transport = new WasmWorkerTransport({ wasm: module, expectedSchemaHash: STUB.SCHEMA_HASH, worker: worker.host, recovery: { snapshotEveryMs: 0, maxSnapshotBytes: 1024 } });
    const core = track(
      await UndraCore.attach(transport, {
        expectedSchemaHash: STUB.SCHEMA_HASH,
        shared: false,
        adapters: { log: captureLog(), http: null, timer: null },
        recovery: crashRecovery(),
        onCoreRestarted: (event) => restarts.push(event),
        onClose: (error) => closed.push(error),
      }),
    );
    core.mirror.register(STUB.HANDLE, () => {});
    await core.observe(STUB.HANDLE, ALL_SIGNALS, true);
    await macrotask();
    const failure = await core.call(FREE, STUB.PANIC, new Uint8Array(0)).catch((e: unknown) => e);
    expect(failure).toMatchObject({ reason: "restarted" });
    await until("the restart", () => restarts.length === 1);
    expect(restarts[0]?.restoredFromAgeMs).not.toBeNull();
    expect(closed).toEqual([]);
    await expect(core.call(FREE, STUB.ECHO, u32(5))).resolves.toEqual(u32(5));
    core.close();
    worker.close();
  });
});
