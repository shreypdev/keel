import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DbErrorCodec, DbMigrationCodec, DbOpenedCodec, DbRowsCodec, DbValueCodec, HeaderCodec, WsOpenedCodec } from "../src/adapters/codecs.js";
import { OptInPortIds } from "../src/adapters/opt-in-ids.js";
import type { WsMessage } from "../src/adapters/types.js";
import { UndraCallError, UndraUnhandledError } from "../src/call-error.js";
import { type CallbackInterface, callbacks, lend } from "../src/callbacks.js";
import { dbPort, nodeSqliteDb } from "../src/db.js";
import { type WebSocketAdapter, webSocketPort } from "../src/realtime.js";
import { UndraCore } from "../src/core.js";
import { streams } from "../src/stream-support.js";
import { UndraTransportError } from "../src/errors.js";
import { adopt, collected } from "../src/identity.js";
import { UndraStore } from "../src/object.js";
import type { PortImpl } from "../src/port.js";
import {
  DEFAULT_RECOVERY,
  type RestartResult,
  type Restartable,
  SnapshotKeeper,
  UndraCoreRestarted,
  emptySnapshot,
  crashRecovery,
  resolveRecovery,
  restartHere,
  snapshotStoreHandles,
  withGenerationFloor,
} from "../src/recovery.js";
import { Signal } from "../src/signal.js";
import type { UndraPanicReport } from "../src/adapters/types.js";
import type { TransportHandler } from "../src/transport/transport.js";
import { WasmMainTransport } from "../src/transport/wasm-main-transport.js";
import { WasmWorkerTransport } from "../src/transport/wasm-worker.js";
import {
  ALL_SIGNALS,
  CallTarget,
  ChangeOp,
  Kind,
  PortStatus,
  UndraWriter,
  codecs,
  decodeSnapshot,
  decodeValue,
  encodeSnapshot,
  encodeValue,
  makeHandle,
} from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { captureLog, deferred, macrotask, microtasks, track } from "./support/harness.js";
import { internalValue, method } from "./support/internals.js";
import { args } from "./support/port-calls.js";
import { CounterStore, u32 } from "./support/store.js";
import { STUB, compileStub } from "./support/stub-core.js";
import { channelWorker } from "./support/worker.js";

/*
 * Recovering a web core that trapped (ADR-049 decision 3): the options, the snapshot keeper, the restart sequence of UndraCore over a scripted transport (every step and its
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

/** A query handle as `undra-bindgen` writes one (ADR-059): a store the core's snapshot names, which it re-issues on its handle. */
class QueryStore extends UndraStore {
  readonly data = new Signal<number>(0);

  private constructor(core: UndraCore, handle: bigint) {
    super(core, handle);
    this._signals = [this.data];
  }

  static async create(core: UndraCore, key: number): Promise<QueryStore> {
    const handle = await core.construct(QUERY_TYPE, QUERY_NEW, u32(key));
    const store = new QueryStore(core, handle);
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

/**
 * A scripted transport that can restart: each restart takes the next scripted outcome, else succeeds with the handles in
 * `restoredStores` (the snapshot's records: its stores and its query handles' recreation records, ADR-059).
 */
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
      // What the generated entry of a schema with a stream passes (ADR-057): the stream this test leaves in flight is open, not loading.
      features: [streams],
      onPanic: (report) => panics.push(report),
      onCoreRestarted: (event) => restarts.push(event),
      onError: (error) => errors.push(error),
      onClose: (error) => closed.push(error),
      ...(options.ports && { ports: options.ports }),
    }),
  );
  return { fake, core, log, panics, restarts, errors, closed, issue };
}

/** The core's internals the recovery tests drive (`@internal`: the production build renames them, `support/internals.ts`). */
const giveBack = (core: UndraCore, handle: bigint): void => method<[bigint], void>(core, "_giveBack")(handle);
const eraOf = (core: UndraCore): number => internalValue<number>(core, "_era");

// A hang detector: the wait is for a condition, and a count of macrotasks (200 of them, which a slow machine runs in the time a restart needs) was a bound on speed.
// It stays under the 5 s a test is given.
const until = async (what: string, probe: () => boolean): Promise<void> => {
  const deadline = Date.now() + 4_000;
  while (!probe()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await macrotask();
  }
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

  // ADR-059: a query handle is in the snapshot as a recreation record, a record with one field whose id is reserved and whose
  // bytes are the core's own. The core re-issues such a handle on restore as it restores a store, so the runtime has to
  // count it among the handles that came back: one that is not in the list is stale, and its wrapper is not observed again.
  const RECREATION_FIELD = 0xffff_fffe;
  const query = makeHandle(5, 3);
  const withQuery = encodeSnapshot({
    generationFloor: 3,
    schemaHash: 9n,
    types: [
      { typeId: 1, fingerprint: 2n },
      { typeId: QUERY_TYPE, fingerprint: 0n },
    ],
    description: "{}",
    stores: [
      { handle: makeHandle(4, 2), typeId: 1, signals: [{ signalId: 0, value: u32(5) }] },
      { handle: query, typeId: QUERY_TYPE, signals: [{ signalId: RECREATION_FIELD, value: Uint8Array.of(1, 0, 0, 0, 0, 0, 0) }] },
    ],
  });

  it("list the recreation record of a query handle with the stores", () => {
    expect(snapshotStoreHandles(withQuery)).toEqual([makeHandle(4, 2), query]);
  });

  it("restartHere reports the handles of the snapshot it restored, the query handle's among them", async () => {
    const keeper = new SnapshotKeeper({ snapshotEveryMs: 0, maxSnapshotBytes: 1024 }, { take: () => withQuery, tooLarge: () => {}, failed: () => {} });
    expect(keeper.takeNow()).toBe(withQuery.byteLength);
    const hello = { undraVersion: "x", schemaHash: 9n, platform: "p", mode: "m" };
    const restored: Uint8Array[] = [];
    const twin = {
      start: () => Promise.resolve(hello),
      restore: (bytes: Uint8Array) => {
        restored.push(bytes);
        return Promise.resolve();
      },
    } as unknown as Restartable;
    const result = await restartHere(twin, {} as TransportHandler, keeper, 8, () => {});
    expect(result.storeHandles).toEqual([makeHandle(4, 2), query]);
    expect(result.restoredFromAgeMs).not.toBeNull();
    expect(restored).toHaveLength(1);
    expect(decodeSnapshot(restored[0] as Uint8Array).generationFloor, "the floor was raised over the handles the host holds").toBe(8);
    expect(decodeSnapshot(restored[0] as Uint8Array).stores.map((record) => record.handle)).toEqual([makeHandle(4, 2), query]);
  });
});

describe("the restart sequence of UndraCore (ADR-049 decision 3.4)", () => {
  it("panic report, then in-flight calls and streams fail 'restarted', restart, re-observe the stores and the query handles, then onCoreRestarted and onError", async () => {
    const t = await recovering();
    const { fake, core } = t;
    // A store and a query handle the snapshot will bring back, and an object that is neither.
    const counterHandle = makeHandle(50, 3);
    fake.store(counterHandle, new Map([[0, u32(1)]]));
    const counter = await CounterStore.create(core, counterHandle);
    const query = await QueryStore.create(core, 4);
    expect(query.data.peek()).toBe(40);
    const queryHandle = query.handle;
    const object = await core.construct(OBJECT_TYPE, OBJECT_NEW, new Uint8Array(0));
    fake.restoredStores = [counterHandle, queryHandle];

    const pending = core.call(FREE, PENDING, new Uint8Array(0));
    // A stream the core has not answered yet: its first item waits.
    const ending = core.stream(FREE, STREAM, new Uint8Array(0))[Symbol.asyncIterator]().next();
    const order: string[] = [];
    pending.catch((error: unknown) => order.push(`call ${(error as UndraTransportError).reason}`));
    ending.catch((error: unknown) => order.push(`stream ${(error as UndraTransportError).reason}`));
    fake.setSignal(counterHandle, 0, u32(9)); // a change the restored snapshot does not have
    await until("the change", () => counter.count.peek() === 9);
    fake.store(counterHandle, new Map([[0, u32(7)]])); // what the restored core answers an observe with
    fake.store(queryHandle, new Map([[0, u32(41)]])); // and for the query handle it builds again when it is observed

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
    // 5. The query handle is in the snapshot as a recreation record (ADR-059): the core re-issued it on its own handle, and
    // the runtime observes it again like a store. The wrapper is untouched, no constructor ran again, and the observe was
    // answered with what the core has now.
    expect(query.handle).toBe(queryHandle);
    expect(core.mirror.has(queryHandle)).toBe(true);
    const ctors = fake.calls.filter((c) => "methodId" in c && c.methodId === QUERY_NEW);
    expect(ctors).toHaveLength(1);
    expect(fake.observed.filter((o) => o.handle === queryHandle && o.on), "observed once when it was made and once more after the restart").toHaveLength(2);
    expect(query.data.peek()).toBe(41);
    fake.setSignal(queryHandle, 0, u32(42));
    await until("the query's change", () => query.data.peek() === 42);
    // 6. One event, to both hooks: the panic, the snapshot's age, the failed calls and the stale object.
    const event = t.restarts[0] as UndraCoreRestarted;
    expect(event).toBeInstanceOf(UndraCoreRestarted);
    expect(event).toBeInstanceOf(UndraUnhandledError);
    expect(event, "the query handle is in the snapshot, so it is not stale").toMatchObject({ restoredFromAgeMs: 250, rejectedCalls: 2, staleObjects: 1, operation: "wasm core" });
    expect(event.message).toBe("the wasm core trapped (kaboom) and was restarted from a snapshot 250 ms old");
    expect(event.report).toBe(t.panics[0]);
    expect(event.error).toBeInstanceOf(UndraCallError.Panicked);
    expect(t.errors).toEqual([event]);
    expect(t.closed).toEqual([]);
    expect(core.closed).toBe(false);
    expect(core.hello.undraVersion).toBe("fake-2");
    // The core works; the object that is neither a store nor a query handle went stale: it is not observed again.
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

  it("replays the releases sent while the core restarted as the references they were: a give-back of a live wrapper's handle is dropped, not sent as a full release", async () => {
    const t = await recovering();
    const gate = deferred<RestartResult>();
    t.fake.outcomes.push(() => gate.promise);
    // A live wrapper of a store (one wrapper per handle, observed) ...
    const live = makeHandle(61, 1);
    t.fake.store(live, new Map([[0, u32(5)]]));
    const wrapper = adopt(t.core, live, CounterStore);
    await t.core.observe(live, ALL_SIGNALS, true);
    expect(wrapper.count.peek()).toBe(5);
    // ... a handle two wrappers closed or gave back ...
    const other = makeHandle(62, 1);
    t.fake.store(other, new Map([[0, u32(1)]]));
    t.fake.trap();
    await macrotask();
    await expect(t.core.call(FREE, ECHO, u32(1))).rejects.toMatchObject({ reason: "restarted" });
    // ... and what happens while the core restarts: a reply carried the live handle again (adopt gives the extra
    // reference back), a superseded wrapper's finalizer did the same, and another handle's wrappers released twice.
    giveBack(t.core, live);
    giveBack(t.core, live);
    giveBack(t.core, other);
    giveBack(t.core, other);
    expect(t.fake.released, "held back while the core restarts").toEqual([]);
    gate.resolve({
      hello: { undraVersion: "x", schemaHash: SCHEMA, platform: "p", mode: "m" },
      restoredFromAgeMs: 10,
      storeHandles: [live, other],
    });
    await until("the restart", () => t.restarts.length === 1);
    // The live wrapper's handle was not released (the restored core counts what the snapshot held, which may not
    // include those extra references), and its routing and observation survive; the other handle's two went out.
    expect(t.fake.released).toEqual([other, other]);
    expect(wrapper.closed).toBe(false);
    expect(t.core.mirror.has(live)).toBe(true);
    t.fake.setSignal(live, 0, u32(6));
    await until("the live wrapper's change", () => wrapper.count.peek() === 6);
    expect(t.fake.observed.filter((o) => o.handle === live && o.on).length, "observed again after the restart").toBeGreaterThan(1);
  });

  it("a wrapper from before a restart does not give back a reference a newer wrapper of its handle may own", async () => {
    const t = await recovering();
    const handle = makeHandle(63, 1);
    t.fake.store(handle, new Map([[0, u32(1)]]));
    const wrapper = adopt(t.core, handle, CounterStore);
    await t.core.observe(handle, ALL_SIGNALS, true);
    const born = eraOf(t.core);
    t.fake.trap();
    await until("the restart", () => t.restarts.length === 1);
    expect(eraOf(t.core)).toBe(born + 1);
    // The finalizer of a wrapper made before the restart runs now: a live wrapper holds the handle, whose count the
    // restored core took from the snapshot.
    collected(t.core, handle, born);
    expect(t.fake.released, "the pre-restart finalizer leaks rather than frees the live wrapper").toEqual([]);
    // A wrapper made after the restart gives back normally.
    collected(t.core, handle, eraOf(t.core));
    expect(t.fake.released).toEqual([handle]);
    // Nothing wraps a handle: a full release whatever the epoch.
    t.fake.released.length = 0;
    collected(t.core, 999n, born);
    expect(t.fake.released).toEqual([999n]);
    expect(wrapper.closed).toBe(false);
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

  // Review (2026-10-02): the new instance can trap after its restore answered but before the restart did (a task the
  // restore woke panics on its first poll; in wasm-worker the worker posts `closed` before `restarted`). That trap
  // arrives while the restart is under way; it was dropped, leaving a dead core that answered "restarted" for ever.
  it("a trap the new instance reports while its restart is still under way restarts it again", async () => {
    const t = await recovering({ recovery: { maxRestarts: 3 } });
    t.fake.outcomes.push(async () => {
      t.fake.trap("the restored state panics in a task");
      // The trap is delivered while the restart is still under way.
      for (let i = 0; i < 5; i++) await macrotask();
      return { hello: { undraVersion: "x", schemaHash: SCHEMA, platform: "p", mode: "m" }, restoredFromAgeMs: 5, storeHandles: [] };
    });
    t.fake.trap("first");
    await until("the second restart", () => t.fake.floors.length === 2);
    await until("the event", () => t.restarts.length === 1);
    expect(t.panics.map((p) => p.message)).toEqual(["first", "the restored state panics in a task"]);
    expect(t.restarts[0]?.report.message).toBe("the restored state panics in a task");
    expect(t.closed).toEqual([]);
    await expect(t.core.call(FREE, ECHO, u32(2))).resolves.toEqual(u32(2));
  });

  it("a trap during the restart that spends the budget ends the core and onClose hears it", async () => {
    const t = await recovering({ recovery: { maxRestarts: 1 } });
    t.fake.outcomes.push(async () => {
      t.fake.trap("again");
      for (let i = 0; i < 5; i++) await macrotask();
      return { hello: { undraVersion: "x", schemaHash: SCHEMA, platform: "p", mode: "m" }, restoredFromAgeMs: 5, storeHandles: [] };
    });
    t.fake.trap("first");
    await until("the close", () => t.closed.length === 1);
    expect(t.closed[0]).toMatchObject({ reason: "trap" });
    expect(t.core.closed).toBe(true);
    expect(t.restarts).toEqual([]);
  });

  // Review (2026-10-02): the floor of a restore is the highest generation the host holds, but a restart forgets the
  // handles that went stale while the app's wrappers keep them; the next restart must not go below what it already used
  // (ADR-022), or the new instance can issue a stale wrapper's handle to another object.
  it("the generation floor never goes down from one restart to the next", async () => {
    const t = await recovering();
    await t.core.construct(OBJECT_TYPE, OBJECT_NEW, new Uint8Array(0)); // generation 7, not a store: stale after a restart
    t.fake.trap("first");
    await until("restart 1", () => t.restarts.length === 1);
    expect(t.restarts[0]?.staleObjects).toBe(1);
    t.fake.trap("second");
    await until("restart 2", () => t.restarts.length === 2);
    expect(t.fake.floors).toEqual([7, 7]);
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

  it("a query handle the snapshot has is observed again on its handle; one closed before the restart is not, and one made after the snapshot is stale like a store made then", async () => {
    const t = await recovering();
    const kept = await QueryStore.create(t.core, 1);
    const closed = await QueryStore.create(t.core, 2);
    closed.close();
    const late = await QueryStore.create(t.core, 3);
    const observes = (store: QueryStore): number => t.fake.observed.filter((o) => o.handle === store.handle && o.on).length;
    const [keptHandle, lateHandle] = [kept.handle, late.handle];
    // The last snapshot kept was taken before `late` was made, so it has a record for `kept` and none for `late` or `closed`.
    t.fake.restoredStores = [keptHandle];
    t.fake.trap();
    await until("the restart", () => t.restarts.length === 1);
    expect(kept.handle, "the wrapper is untouched (ADR-059)").toBe(keptHandle);
    expect(observes(kept), "observed again").toBe(2);
    expect(observes(closed), "released, so not observed again").toBe(1);
    expect(observes(late), "not in the snapshot, so not observed again").toBe(1);
    expect(late.handle).toBe(lateHandle);
    expect(t.restarts[0]?.staleObjects, "only the one the snapshot did not have").toBe(1);
    expect(t.fake.calls.filter((c) => "methodId" in c && c.methodId === QUERY_NEW), "none was constructed again").toHaveLength(3);
  });
});

describe("ports that hold platform resources across a restart (ADR-047, ADR-048)", () => {
  const strings = codecs.vec(codecs.string);
  const headers = codecs.vec(HeaderCodec);
  const values = codecs.vec(DbValueCodec);
  const connectArgs = (url: string) =>
    args((w) => {
      w.writeStr(url);
      strings.encode(w, []);
      headers.encode(w, []);
    });
  const statementArgs = (id: number, sql: string) =>
    args((w) => {
      w.writeU32(id);
      w.writeStr(sql);
      values.encode(w, []);
    });
  const openArgs = (name: string) =>
    args((w) => {
      w.writeStr(name);
      codecs.vec(DbMigrationCodec).encode(w, [{ version: 1, sql: "CREATE TABLE t (v INTEGER)" }]);
    });

  it("a restart closes the WebSocket connections of the instance that trapped (1001), and the port serves the new instance", async () => {
    const closes: Array<[number, string]> = [];
    const adapter: WebSocketAdapter = {
      connect: async () => ({
        protocol: "",
        messages: () => ({ [Symbol.asyncIterator]: () => ({ next: () => new Promise<IteratorResult<WsMessage>>(() => {}) }) }),
        send: async () => {},
        close: async (code, reason) => {
          closes.push([code, reason]);
        },
      }),
    };
    const ws = OptInPortIds.WebSocket;
    const t = await recovering({ ports: { [ws.portId]: webSocketPort(adapter) } });
    const first = await t.fake.callPort(ws.portId, ws.connect, connectArgs("ws://old.test/"));
    expect(decodeValue(WsOpenedCodec, first.body).conn).toBe(1);
    t.fake.trap();
    await until("the restart", () => t.restarts.length === 1);
    expect(closes, "the connection of the instance that trapped is closed going away").toEqual([[1001, ""]]);
    const second = await t.fake.callPort(ws.portId, ws.connect, connectArgs("ws://new.test/"));
    expect(second.status, "the port still serves the new instance").toBe(PortStatus.Ok);
    expect(decodeValue(WsOpenedCodec, second.body).conn).toBe(2);
  });

  it("a restart rolls back the transaction the instance that trapped left open: the new instance's begin is not Busy", async () => {
    const directory = mkdtempSync(join(tmpdir(), "undra-restart-db-"));
    try {
      const db = OptInPortIds.Db;
      const t = await recovering({ ports: { [db.portId]: dbPort(nodeSqliteDb({ directory })) } });
      const opened = decodeValue(DbOpenedCodec, (await t.fake.callPort(db.portId, db.open, openArgs("app"))).body);
      const tx = decodeValue(codecs.u32, (await t.fake.callPort(db.portId, db.begin, args((w) => w.writeU32(opened.db)))).body);
      expect((await t.fake.callPort(db.portId, db.execute, statementArgs(tx, "INSERT INTO t VALUES (1)"))).status).toBe(PortStatus.Ok);
      t.fake.trap();
      await until("the restart", () => t.restarts.length === 1);
      const again = decodeValue(DbOpenedCodec, (await t.fake.callPort(db.portId, db.open, openArgs("app"))).body);
      const started = Date.now();
      const begun = await t.fake.callPort(db.portId, db.begin, args((w) => w.writeU32(again.db)));
      expect(begun.status === PortStatus.Ok ? "ok" : decodeValue(DbErrorCodec, begun.body), "BEGIN IMMEDIATE got the write lock").toBe("ok");
      // Not after SQLite's busy timeout (5 s): under half of it. A wait for the lock that the rollback should have freed ends in Busy at 5 s (the status check
      // above), so this only has to tell "got it" from "waited for it"; how fast a slow machine runs the rest is not what it asks.
      expect(Date.now() - started, "at once, not after SQLite's busy timeout").toBeLessThan(2_500);
      const tx2 = decodeValue(codecs.u32, begun.body);
      const rows = decodeValue(DbRowsCodec, (await t.fake.callPort(db.portId, db.query, statementArgs(tx2, "SELECT COUNT(*) FROM t"))).body);
      expect(rows.rows, "the uncommitted row is gone").toEqual([[{ kind: "integer", value: 0n }]]);
      expect((await t.fake.callPort(db.portId, db.commit, args((w) => w.writeU32(tx2)))).status).toBe(PortStatus.Ok);
      expect((await t.fake.callPort(db.portId, db.close, args((w) => w.writeU32(again.db)))).status).toBe(PortStatus.Ok);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  }, 15_000);
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

  it("wasm-main with recovery keeps the in-process fast path: calls take sendCall and callSyncParts, before and after a restart (ADR-056, ADR-057)", async () => {
    const module = await WebAssembly.compile((await compileStub({ snapshot: true })) as Uint8Array<ArrayBuffer>);
    const transport = new WasmMainTransport({ wasm: module, expectedSchemaHash: STUB.SCHEMA_HASH });
    const sendCall = vi.spyOn(transport, "sendCall");
    const callSyncParts = vi.spyOn(transport, "callSyncParts");
    const send = vi.spyOn(transport, "send");
    const restarts: UndraCoreRestarted[] = [];
    const core = track(
      await UndraCore.attach(transport, {
        expectedSchemaHash: STUB.SCHEMA_HASH,
        shared: false,
        adapters: { log: captureLog() },
        recovery: crashRecovery({ snapshotEveryMs: 0, maxSnapshotBytes: 1024 }),
        onCoreRestarted: (event) => restarts.push(event),
      }),
    );
    await expect(core.call(FREE, STUB.ECHO, u32(1))).resolves.toEqual(u32(1));
    expect(core.callSync(FREE, STUB.ECHO, u32(2))).toEqual(u32(2));
    expect([sendCall.mock.calls.length, callSyncParts.mock.calls.length, send.mock.calls.length]).toEqual([1, 1, 0]);
    await core.call(FREE, STUB.PANIC, new Uint8Array(0)).catch(() => {});
    await until("the restart", () => restarts.length === 1);
    // The twin is a new instance of the same class: its own calls are not the spied ones, and nothing used `send`.
    await expect(core.call(FREE, STUB.ECHO, u32(3))).resolves.toEqual(u32(3));
    expect(core.callSync(FREE, STUB.ECHO, u32(4))).toEqual(u32(4));
    expect(send).not.toHaveBeenCalled();
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

  it("wasm-worker: the ports of the worker's ports module release what the instance held at a restart, and when the worker closes", async () => {
    const url = new URL("./support/worker-ports/disposing.mjs", import.meta.url).href;
    const { disposed } = (await import(url)) as { disposed: string[] };
    disposed.length = 0;
    const module = await WebAssembly.compile((await compileStub({ snapshot: true })) as Uint8Array<ArrayBuffer>);
    const worker = channelWorker();
    const restarts: UndraCoreRestarted[] = [];
    const transport = new WasmWorkerTransport({
      wasm: module,
      expectedSchemaHash: STUB.SCHEMA_HASH,
      worker: worker.host,
      ports: url,
      recovery: { snapshotEveryMs: 0, maxSnapshotBytes: 1024 },
    });
    const core = track(
      await UndraCore.attach(transport, {
        expectedSchemaHash: STUB.SCHEMA_HASH,
        shared: false,
        adapters: { log: captureLog(), http: null, timer: null },
        recovery: crashRecovery(),
        onCoreRestarted: (event) => restarts.push(event),
      }),
    );
    await expect(core.call(FREE, STUB.PANIC, new Uint8Array(0))).rejects.toMatchObject({ reason: "restarted" });
    await until("the restart", () => restarts.length === 1);
    expect(disposed, "released at the restart").toEqual(["dispose"]);
    core.close();
    await until("the worker to close", () => disposed.length === 2);
    worker.close();
  });

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

describe("host callbacks across a restart (ADR-041)", () => {
  const PORT = 0x7000_0101;
  const NOTE = 0x21;
  const ASK = 0x22;
  interface Listener {
    note(line: string): void;
    ask(question: string, signal: AbortSignal): Promise<boolean>;
  }
  const spec: CallbackInterface<Listener> = {
    name: "Listener",
    portId: PORT,
    releaseInstance: 0x2e,
    cancelCall: 0x2f,
    methods: {
      [NOTE]: {
        name: "note",
        notify: (r) => {
          const line = r.readStr();
          return (impl) => impl.note(line);
        },
      },
      [ASK]: {
        name: "ask",
        call: (r) => {
          const question = r.readStr();
          return async (impl, signal) => encodeValue(codecs.bool, await impl.ask(question, signal));
        },
      },
    },
  };
  const into = (instance: bigint, text: string): Uint8Array => {
    const w = new UndraWriter();
    w.writeU64(instance);
    w.writeStr(text);
    return w.finish();
  };

  it("a restart drops the callbacks the instance that trapped held: what runs is aborted, what it queued is not delivered", async () => {
    const t = await recovering();
    const heard: string[] = [];
    let aborted = false;
    const listener: Listener = {
      note: (line) => heard.push(line),
      ask: (_question, signal) =>
        new Promise((_, reject) => {
          signal.addEventListener("abort", () => {
            aborted = true;
            reject(signal.reason);
          });
        }),
    };
    const instance = lend(t.core, listener, spec);
    void t.fake.callPort(PORT, ASK, into(instance, "go on?"));
    await macrotask();
    expect(callbacks(t.core).liveCount).toBe(1);
    t.fake.burst(() => {
      void t.fake.notifyPort(PORT, NOTE, into(instance, "queued by the instance that trapped"));
      t.fake.trap();
    });
    await until("the restart", () => t.restarts.length === 1);
    await macrotask();
    expect(callbacks(t.core).liveCount, "the restored instance holds none of them").toBe(0);
    expect(aborted, "the running implementation's signal aborted").toBe(true);
    expect(heard, "nothing the trapped instance queued reaches the app").toEqual([]);
    // Lent again, the listener is a new instance of the new core.
    expect(lend(t.core, listener, spec)).not.toBe(instance);
    expect(callbacks(t.core).liveCount).toBe(1);
  });
});
