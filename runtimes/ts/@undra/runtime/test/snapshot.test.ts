import { describe, expect, it } from "vitest";
import { UndraCore } from "../src/core.js";
import { UndraError, UndraModeError, UndraReplyError, UndraRestoreError, UndraTransportError } from "../src/errors.js";
import type { PortImpl } from "../src/port.js";
import { WasmMainTransport } from "../src/transport/wasm-main.js";
import { WasmWorkerTransport, type WorkerLike } from "../src/transport/wasm-worker.js";
import type { Transport, TransportHandler } from "../src/transport/transport.js";
import { ALL_SIGNALS, CallTarget, Kind, ReplyStatus, decodeReply, encodeCall } from "../src/wire/index.js";
import { FakeCoreTransport } from "./support/fake-core.js";
import { track } from "./support/harness.js";
import { STUB, compileStub, stubGlobals } from "./support/stub-core.js";
import { u32 } from "./support/store.js";
import { channelWorker } from "./support/worker.js";

/*
 * `UndraCore.snapshot()` and `restore()` (docs/SPEC.md sections 5.9, 11 and 17.1; gap PA-5), in `wasm-main`
 * and in `wasm-worker` mode, against the stub core of test/support/stub-core.ts built with its snapshot
 * exports. The real core, with real stores, runs in crates/undra-ffi/tests/wasm/ts-runtime.test.mjs (both
 * modes, the worker on a real worker thread) and in contract scenario S15.
 */

const SCHEMA = STUB.SCHEMA_HASH;
const FREE = CallTarget.FreeFunction;

/** The restored value of the stub's store: the little-endian `u32` at the start of the snapshot. */
const snapshotOf = (value: number): Uint8Array => Uint8Array.of(...u32(value), 9, 9, 9, 9);
const REFUSED_MALFORMED = Uint8Array.of(0xff, 0, 0, 0, 0);
const REFUSED_UNAVAILABLE = Uint8Array.of(0xfe, 0, 0, 0, 0);

interface Booted {
  readonly core: UndraCore;
  readonly transport: Transport;
  /** What the observed store of the stub (`STUB.HANDLE`, signal 0) was last told, and every value in order. */
  readonly seen: number[];
  /** Runs the mirror's scheduled drains (the test uses a manual scheduler: only `flush()` or this applies a change-set). */
  runDrains(): void;
  /** A port that never answers: calls to `STUB.PORT` wait in the core. */
  readonly ports: Readonly<Record<number, PortImpl>>;
  close(): void;
}

const NEVER: PortImpl = { sync: false, methods: { [STUB.PORT_METHOD]: () => new Promise<Uint8Array>(() => {}) } };

async function boot(mode: "wasm-main" | "wasm-worker", stub: Parameters<typeof compileStub>[0] = { snapshot: true }): Promise<Booted> {
  const module = await WebAssembly.compile((await compileStub(stub)) as Uint8Array<ArrayBuffer>);
  const drains: Array<() => void> = [];
  let transport: Transport;
  let stop = (): void => {};
  if (mode === "wasm-main") {
    transport = new WasmMainTransport({ wasm: module, expectedSchemaHash: SCHEMA, platform: "test" });
  } else {
    const worker = channelWorker();
    stop = () => {
      worker.close();
    };
    transport = new WasmWorkerTransport({ wasm: module, expectedSchemaHash: SCHEMA, worker: worker.host });
  }
  const ports = { [STUB.PORT_ID]: NEVER };
  const core = track(
    await UndraCore.attach(transport, {
      expectedSchemaHash: SCHEMA,
      shared: false,
      adapters: { log: { log() {} }, http: null, timer: null },
      mirror: {
        schedule: (fn) => {
          drains.push(fn);
        },
      },
      ports,
    }),
  );
  const seen: number[] = [];
  core.mirror.register(STUB.HANDLE, (_signalId, _op, value) => {
    seen.push(new DataView(value.buffer, value.byteOffset, value.byteLength).getUint32(0, true));
  });
  const observed = core.observe(STUB.HANDLE, ALL_SIGNALS, true);
  await observed;
  return {
    core,
    transport,
    seen,
    runDrains() {
      while (drains.length > 0) drains.shift()?.();
    },
    ports,
    close() {
      core.close();
      stop();
    },
  };
}

describe.each(["wasm-main", "wasm-worker"] as const)("snapshot and restore in %s mode", (mode) => {
  it("snapshot resolves with the core's bytes, a copy the caller owns", async () => {
    const b = await boot(mode);
    const first = await b.core.snapshot();
    expect(first).toBeInstanceOf(Uint8Array);
    expect([...first]).toEqual([...STUB.SNAPSHOT]);
    // A copy: changing it changes nothing the next snapshot returns.
    first.fill(0);
    expect([...(await b.core.snapshot())]).toEqual([...STUB.SNAPSHOT]);
    if (mode === "wasm-main") {
      const globals = stubGlobals((b.transport as WasmMainTransport).instance as WebAssembly.Instance);
      expect(globals.buf_free_count, "every UndraBuf the core handed out was released").toBeGreaterThanOrEqual(2);
    }
    b.close();
  });

  it("restore resolves after the restored values reached the stores", async () => {
    const b = await boot(mode);
    b.runDrains();
    expect(b.seen).toEqual([42]); // the initial value of the observation
    await b.core.restore(snapshotOf(77));
    // Not a microtask, not a drain later: the promise resolved after the mirror applied the restored change-set
    // (the test's scheduler never runs a drain by itself, so only restore's own flush can have done it).
    expect(b.seen).toEqual([42, 77]);
    expect(b.core.mirror.pending).toBe(0);
    await b.core.restore(snapshotOf(78));
    expect(b.seen).toEqual([42, 77, 78]);
    b.close();
  });

  it("restore leaves the caller's bytes alone", async () => {
    const b = await boot(mode);
    const bytes = snapshotOf(5);
    await b.core.restore(bytes);
    expect(bytes.byteLength).toBe(8);
    expect([...bytes]).toEqual([...snapshotOf(5)]);
    expect(bytes.buffer.byteLength, "the buffer was not transferred away").toBeGreaterThanOrEqual(8);
    b.close();
  });

  it.each([
    ["malformed bytes", REFUSED_MALFORMED, 5],
    ["a core that is not available", REFUSED_UNAVAILABLE, 6],
    ["bytes too short to be a snapshot", Uint8Array.of(1, 2), 5],
    ["no bytes at all", new Uint8Array(0), 5],
  ] as const)("refused bytes (%s) reject with UndraRestoreError carrying the code, and the core is unchanged", async (_name, bytes, code) => {
    const b = await boot(mode);
    b.runDrains();
    const refusal = await b.core.restore(bytes).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(refusal).toBeInstanceOf(UndraRestoreError);
    expect(refusal).toBeInstanceOf(UndraError);
    const error = refusal as UndraRestoreError;
    expect(error.code).toBe(code);
    expect(error.kind).toBe("restore");
    expect(error.name).toBe("UndraRestoreError");
    expect(error.message).toContain(`code ${code}`);
    expect(error.message).toContain("unchanged");
    // Nothing was delivered, the core still runs and answers.
    b.runDrains();
    expect(b.seen).toEqual([42]);
    expect(b.core.closed).toBe(false);
    const echoed = await b.core.call(FREE, STUB.ECHO, u32(3));
    expect([...echoed]).toEqual([...u32(3)]);
    // And a good snapshot restores afterwards.
    await b.core.restore(snapshotOf(11));
    expect(b.seen).toEqual([42, 11]);
    b.close();
  });

  it("a call waiting on the core when the restore runs ends as cancelled by the core", async () => {
    const b = await boot(mode);
    const waiting = b.core.call(FREE, STUB.PORT, u32(1)); // the port never answers
    waiting.catch(() => {});
    // In worker mode the call is posted; the restore request follows it on the same channel.
    await b.core.restore(snapshotOf(3));
    const error = await waiting.then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(UndraReplyError);
    expect((error as UndraReplyError).status).toBe(ReplyStatus.Cancelled);
    expect(b.core.closed).toBe(false);
    b.close();
  });

  it("a core without the exports rejects typed instead of waiting", async () => {
    const b = await boot(mode, {});
    const snapshot = await b.core.snapshot().then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(snapshot).toBeInstanceOf(UndraTransportError);
    expect((snapshot as UndraTransportError).reason).toBe("unsupported");
    expect((snapshot as UndraTransportError).message).toContain("undra_snapshot");
    const restore = await b.core.restore(snapshotOf(1)).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(restore).toBeInstanceOf(UndraTransportError);
    expect((restore as UndraTransportError).reason).toBe("unsupported");
    expect((restore as UndraTransportError).message).toContain("undra_restore");
    expect(b.core.closed).toBe(false);
    b.close();
  });

  it("a closed core rejects with UndraTransportError('closed')", async () => {
    const b = await boot(mode);
    b.core.close();
    for (const operation of [() => b.core.snapshot(), () => b.core.restore(snapshotOf(1))]) {
      const error = await operation().then(
        () => undefined,
        (e: unknown) => e,
      );
      expect(error).toBeInstanceOf(UndraTransportError);
      expect((error as UndraTransportError).reason).toBe("closed");
    }
    b.close();
  });

  it("a core closed while the request is on its way rejects it", async () => {
    const b = await boot(mode);
    const pending = b.core.snapshot();
    const settled = pending.then(
      (bytes) => bytes,
      (e: unknown) => e,
    );
    b.core.close();
    const outcome = await settled;
    // wasm-main answers inside `snapshot()` before `close()` can run; the worker's answer is still in flight.
    if (outcome instanceof Uint8Array) expect(mode).toBe("wasm-main");
    else expect((outcome as UndraTransportError).reason).toBe("closed");
    b.close();
  });
});

describe("a transport that cannot snapshot", () => {
  it("rejects with UndraModeError naming the operation and the mode", async () => {
    const fake = new FakeCoreTransport({ synchronous: false, mode: "remote" });
    const core = track(await UndraCore.attach(fake, { expectedSchemaHash: fake.schemaHash, shared: false, adapters: { log: { log() {} }, http: null, timer: null } }));
    for (const [name, run] of [
      ["snapshot", () => core.snapshot()],
      ["restore", () => core.restore(snapshotOf(1))],
    ] as const) {
      const error = await run().then(
        () => undefined,
        (e: unknown) => e,
      );
      expect(error, name).toBeInstanceOf(UndraModeError);
      expect((error as UndraModeError).operation).toBe(name);
      expect((error as UndraModeError).mode).toBe("remote");
    }
    expect(core.closed).toBe(false);
    core.close();
  });

  it("a closed core says so before it says anything about the mode", async () => {
    const fake = new FakeCoreTransport({ synchronous: false, mode: "remote" });
    const core = track(await UndraCore.attach(fake, { expectedSchemaHash: fake.schemaHash, shared: false, adapters: { log: { log() {} }, http: null, timer: null } }));
    core.close();
    const error = await core.snapshot().then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(UndraTransportError);
  });
});

describe("a raw Restore envelope still works on the main-thread transport", () => {
  it("send(Kind.Restore) restores, and a refusal throws the typed error", async () => {
    const module = await WebAssembly.compile((await compileStub({ snapshot: true })) as Uint8Array<ArrayBuffer>);
    const transport = new WasmMainTransport({ wasm: module, expectedSchemaHash: SCHEMA, platform: "test" });
    const changes: number[] = [];
    await transport.start({
      reply: () => {},
      changeSet: () => {
        changes.push(1);
      },
      streamItem: () => {},
      portCall: () => ({ kind: "unavailable" }),
      log: () => {},
      closed: () => {},
    });
    transport.send(Kind.Restore, snapshotOf(4));
    expect(changes).toHaveLength(1);
    expect(() => {
      transport.send(Kind.Restore, REFUSED_MALFORMED);
    }).toThrow(UndraRestoreError);
    expect(changes).toHaveLength(1);
  });
});

describe("the worker transport's requests", () => {
  /** A handler that records the order of what the core says. */
  function recorder(): { readonly handler: TransportHandler; readonly events: string[] } {
    const events: string[] = [];
    return {
      events,
      handler: {
        reply: (payload) => {
          events.push(`reply:${decodeReply(payload).status}`);
        },
        changeSet: () => {
          events.push("changeSet");
        },
        streamItem: () => {},
        portCall: () => ({ kind: "async" }),
        log: () => {},
        closed: () => {},
      },
    };
  }

  it("restore resolves after the change-sets and the cancellations it produced were delivered, in that order", async () => {
    const module = await WebAssembly.compile((await compileStub({ snapshot: true })) as Uint8Array<ArrayBuffer>);
    const worker = channelWorker();
    const transport = new WasmWorkerTransport({ wasm: module, expectedSchemaHash: SCHEMA, worker: worker.host });
    const { handler, events } = recorder();
    await transport.start(handler);
    // A call the core cannot finish (its port never answers), then the restore.
    transport.send(Kind.Call, encodeCall({ target: FREE, methodId: STUB.PORT, callId: 1, args: u32(1) }));
    await transport.restore(snapshotOf(8));
    events.push("restore resolved");
    // The restore cancelled the call (status 3) and delivered the observed signal again; both reached the
    // handler before the acknowledgement did, because the worker posts its batch before the control message.
    expect(events.filter((e) => e !== "reply:3" && e !== "changeSet")).toEqual(["restore resolved"]);
    expect(events.indexOf("restore resolved")).toBe(events.length - 1);
    expect(events).toContain("reply:3");
    expect(events).toContain("changeSet");
    transport.close();
    worker.close();
  });

  it("two requests in flight are answered to the right callers", async () => {
    const module = await WebAssembly.compile((await compileStub({ snapshot: true })) as Uint8Array<ArrayBuffer>);
    const worker = channelWorker();
    const transport = new WasmWorkerTransport({ wasm: module, expectedSchemaHash: SCHEMA, worker: worker.host });
    await transport.start(recorder().handler);
    const [snapshot, refused, accepted, again] = await Promise.allSettled([
      transport.snapshot(),
      transport.restore(REFUSED_MALFORMED),
      transport.restore(snapshotOf(1)),
      transport.snapshot(),
    ]);
    expect(snapshot.status).toBe("fulfilled");
    expect(refused.status).toBe("rejected");
    expect((refused as PromiseRejectedResult).reason).toBeInstanceOf(UndraRestoreError);
    expect(accepted.status).toBe("fulfilled");
    expect(again.status).toBe("fulfilled");
    transport.close();
    worker.close();
  });

  /** A worker script by hand: answers `init` with `ready` (with `features`), and nothing else. */
  function silentWorker(features: readonly string[] | undefined): { readonly worker: WorkerLike; readonly received: unknown[] } {
    const listeners: Array<(event: Event) => void> = [];
    const received: unknown[] = [];
    const worker: WorkerLike = {
      addEventListener: (type, fn) => {
        if (type === "message") listeners.push(fn as (event: Event) => void);
      },
      removeEventListener: () => {},
      postMessage: (message) => {
        received.push(message);
        if ((message as { t: string }).t !== "init") return;
        const hello = { undraVersion: "test", schemaHash: SCHEMA, platform: "test", mode: "dev" };
        queueMicrotask(() => {
          for (const fn of listeners) fn({ data: { t: "ready", hello, ...(features !== undefined && { features }) } } as MessageEvent);
        });
      },
      close: () => {},
    };
    return { worker, received };
  }

  it("a worker script that predates snapshots is refused at once, not waited for", async () => {
    const { worker, received } = silentWorker(undefined);
    const transport = new WasmWorkerTransport({ wasm: new Uint8Array(8), expectedSchemaHash: SCHEMA, worker });
    await transport.start(recorder().handler);
    for (const run of [() => transport.snapshot(), () => transport.restore(snapshotOf(1))]) {
      const error = await run().then(
        () => undefined,
        (e: unknown) => e,
      );
      expect(error).toBeInstanceOf(UndraTransportError);
      expect((error as UndraTransportError).reason).toBe("unsupported");
    }
    expect(received.map((m) => (m as { t: string }).t), "nothing but init reached the old worker").toEqual(["init"]);
    transport.close();
  });

  it("a request that is never answered rejects when the transport closes, never hangs it", async () => {
    const { worker } = silentWorker(["snapshot"]);
    const transport = new WasmWorkerTransport({ wasm: new Uint8Array(8), expectedSchemaHash: SCHEMA, worker });
    await transport.start(recorder().handler);
    const snapshot = transport.snapshot();
    const restore = transport.restore(snapshotOf(1));
    transport.close();
    for (const pending of [snapshot, restore]) {
      const error = await pending.then(
        () => undefined,
        (e: unknown) => e,
      );
      expect(error).toBeInstanceOf(UndraTransportError);
      expect((error as UndraTransportError).reason).toBe("closed");
    }
  });

  it("an answer that is not what was asked for is ignored, and a malformed one rejects typed", async () => {
    const listeners: Array<(event: Event) => void> = [];
    let initialised = false;
    const worker: WorkerLike = {
      addEventListener: (type, fn) => {
        if (type === "message") listeners.push(fn as (event: Event) => void);
      },
      removeEventListener: () => {},
      postMessage: (message) => {
        const t = (message as { t: string }).t;
        const emit = (data: unknown): void => {
          queueMicrotask(() => {
            for (const fn of listeners) fn({ data } as MessageEvent);
          });
        };
        if (t === "init") {
          initialised = true;
          emit({ t: "ready", hello: { undraVersion: "test", schemaHash: SCHEMA, platform: "test", mode: "dev" }, features: ["snapshot"] });
        } else if (t === "snapshot") {
          const id = (message as { id: number }).id;
          emit({ t: "restored", id, code: 0 }); // the wrong kind of answer: ignored
          emit({ t: "snapshot", id }); // neither bytes nor a failure
        }
      },
      close: () => {},
    };
    const transport = new WasmWorkerTransport({ wasm: new Uint8Array(8), expectedSchemaHash: SCHEMA, worker });
    await transport.start(recorder().handler);
    expect(initialised).toBe(true);
    const error = await transport.snapshot().then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(UndraTransportError);
    expect((error as UndraTransportError).reason).toBe("protocol");
    transport.close();
  });
});

describe("the envelope kinds", () => {
  it("a Restore envelope sent to the worker is still relayed (no acknowledgement)", async () => {
    // The worker's relay keeps accepting `Kind.Restore` over the envelope path, as it did before the control messages.
    const module = await WebAssembly.compile((await compileStub({ snapshot: true })) as Uint8Array<ArrayBuffer>);
    const worker = channelWorker();
    const transport = new WasmWorkerTransport({ wasm: module, expectedSchemaHash: SCHEMA, worker: worker.host });
    const events: string[] = [];
    await transport.start({
      reply: () => {},
      changeSet: () => {
        events.push("changeSet");
      },
      streamItem: () => {},
      portCall: () => ({ kind: "async" }),
      log: () => {},
      closed: () => {},
    });
    transport.send(Kind.Restore, snapshotOf(6));
    for (let i = 0; i < 100 && events.length === 0; i++) await new Promise((resolve) => setTimeout(resolve, 1));
    expect(events).toEqual(["changeSet"]);
    transport.close();
    worker.close();
  });
});
