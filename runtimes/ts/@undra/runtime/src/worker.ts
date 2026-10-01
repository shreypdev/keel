import { PortIds } from "./adapters/ids.js";
import { WEB_CRYPTO_REQUIRED, hasCryptoRandom } from "./adapters/system.js";
import type { ClockAdapter, RngAdapter, TimerAdapter } from "./adapters/types.js";
import { UndraError, UndraReplyError, UndraRestoreError, UndraSchemaMismatchError, UndraTransportError } from "./errors.js";
import { errorMessage } from "./platform.js";
import type { PortImpl } from "./port.js";
import { dispatchPortCall, portOperation } from "./port-dispatch.js";
import { trapStack } from "./panic.js";
import { type SnapshotKeeper, keeperOf, restartHere } from "./recovery.js";
import type { Transport, TransportHandler } from "./transport/transport.js";
import { WasmMainTransport, type WasmSource } from "./transport/wasm-main.js";
import {
  type HostToWorker,
  WORKER_FEATURES,
  type WorkerFailure,
  type WorkerToHost,
  type WorkerWasm,
} from "./transport/worker-protocol.js";
import {
  Kind,
  ReplyStatus,
  WireError,
  decodeCall,
  decodeEnvelope,
  encodeEnvelope,
  encodeLog,
  encodePortCall,
  encodeReply,
} from "./wire/index.js";

/*
 * The worker side of the `wasm-worker` mode (`@undra/runtime/worker`): runs the
 * core with the very same `WasmMainTransport` the `wasm-main` mode uses and
 * speaks envelopes to the main-thread proxy. Point a module worker at this
 * file (the default of `mode: "wasm-worker"` does) or call `runWorker(self)`
 * from your own worker script.
 *
 * Ports (ADR-049). The core calls synchronous ports while it runs and cannot wait for the main
 * thread, so the worker answers what it can itself: a port implemented in the worker (the default
 * export of the module of `LoadOptions.worker.ports`) is answered here, synchronously or later; a
 * port the host registered with asynchronous methods crosses to the main thread; every other port
 * is "unavailable" (SPEC 7), which makes the core's built-in bindings answer Clock, Rng and Log
 * from the worker's own `Date.now`, `crypto.getRandomValues` and `log` import (the `log` import is
 * relayed to the main thread's `Log` adapter).
 */

/** The ports the wasm shell binds itself over the `now_ms`, `random` and `log` imports (SPEC 7, `builtin.rs`): what a host before protocol 3 left to the worker. */
const BUILT_IN_PORTS: ReadonlySet<number> = new Set([PortIds.Clock.portId, PortIds.Rng.portId, PortIds.Log.portId]);

/**
 * What the module of `LoadOptions.worker.ports` exports (ADR-049). The worker imports it before `undra_init`.
 *
 * ```ts
 * // worker-ports.ts, given as `worker: { ports: new URL("./worker-ports.js", import.meta.url) }`
 * import { PortIds, clockPort } from "@undra/runtime";
 * import { UndraIds, sumPortImpl } from "./generated/index.js";
 * export default {
 *   [UndraIds.Ports.Sum.portId]: sumPortImpl({ add: (a, b) => a + b }), // an app's sync port, answered in the worker
 *   [PortIds.Clock.portId]: clockPort(myClock),                          // overrides the built-in clock
 * };
 * export const adapters = { timer: myTimer };                             // backs the core's timers
 * ```
 */
export interface WorkerPortsModule {
  /** Port implementations by port id, as `UndraCore.registerPort` takes them; they run in the worker, synchronous ones synchronously. */
  readonly default?: Readonly<Record<number, PortImpl>>;
  /**
   * What backs the wasm imports in the worker, as `adapters` does in `wasm-main`: `clock` behind `now_ms`, `rng` behind
   * `random` (it must fill with cryptographically secure bytes or throw), `timer` behind `timer_set` (the core's timers).
   */
  readonly adapters?: { readonly clock?: ClockAdapter; readonly rng?: RngAdapter; readonly timer?: TimerAdapter };
}

/** Whether `value` looks like a `PortImpl`. */
function isPortImpl(value: unknown): value is PortImpl {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as { sync?: unknown }).sync === "boolean" &&
    typeof (value as { methods?: unknown }).methods === "object" &&
    (value as { methods?: unknown }).methods !== null
  );
}

/** Imports and checks the module of `LoadOptions.worker.ports`. */
async function loadPortsModule(url: string): Promise<{ ports: Map<number, PortImpl>; adapters: NonNullable<WorkerPortsModule["adapters"]> }> {
  let loaded: WorkerPortsModule;
  try {
    loaded = (await import(/* @vite-ignore */ /* webpackIgnore: true */ url)) as WorkerPortsModule;
  } catch (cause) {
    throw new UndraTransportError("handshake", `the worker could not import the ports module ${url}: ${errorMessage(cause)}`, { cause });
  }
  const ports = new Map<number, PortImpl>();
  const table: unknown = loaded.default ?? {};
  if (typeof table !== "object" || table === null) {
    throw new UndraTransportError("handshake", `the default export of the ports module ${url} must map port ids to implementations`);
  }
  for (const [key, impl] of Object.entries(table)) {
    const portId = Number(key);
    if (!Number.isInteger(portId) || portId < 0 || portId > 0xffff_ffff || !isPortImpl(impl)) {
      throw new UndraTransportError(
        "handshake",
        `the ports module ${url} maps ${JSON.stringify(key)} to something that is not a port implementation ({ sync, methods }, as registerPort takes)`,
      );
    }
    ports.set(portId, impl);
  }
  return { ports, adapters: loaded.adapters ?? {} };
}

/** What the worker needs of its global scope; a `DedicatedWorkerGlobalScope` or one end of a `MessageChannel`. */
export interface WorkerScope extends Pick<EventTarget, "addEventListener" | "removeEventListener"> {
  postMessage(message: unknown, transfer?: Transferable[]): void;
}

function toFailure(error: unknown): WorkerFailure {
  if (error instanceof UndraSchemaMismatchError) {
    return { kind: "schemaMismatch", expected: error.expected, got: error.got };
  }
  if (error instanceof UndraTransportError) {
    // A trap's stack (the engine's frames) travels for the panic report (ADR-046 decision 4.4).
    const stack = error.reason === "trap" ? trapStack(error) : "";
    return { kind: "transport", reason: error.reason, message: error.message, ...(stack !== "" && { stack }) };
  }
  return { kind: "error", message: errorMessage(error) };
}

/** Why there is nothing to snapshot or restore: the core is gone, or its transport cannot. */
function closedFailure(transport: Transport | null): UndraTransportError {
  return transport === null
    ? new UndraTransportError("closed", "the core is closed")
    : new UndraTransportError("unsupported", "the worker's transport cannot snapshot or restore");
}

function wasmSource(wasm: WorkerWasm): WasmSource {
  switch (wasm.kind) {
    case "url":
      return new URL(wasm.href);
    case "bytes":
      return wasm.bytes;
    case "module":
      return wasm.module;
  }
}

/**
 * Starts serving the main thread on `scope`: waits for `init`, loads the core,
 * then relays envelopes. Returns a function that stops listening and closes
 * the core.
 *
 * The envelopes the core produces during one task of this worker (a burst of
 * change-sets and the reply that follows them) travel to the main thread as one
 * `envelopes` message, posted from a microtask, so the main thread pays one
 * task for the burst instead of one per change-set (ADR-031). A host that did
 * not announce protocol 2 gets one `envelope` message each, as before.
 */
export function runWorker(scope: WorkerScope): () => void {
  let transport: Transport | null = null;
  /** The transport as a restartable one (the same object), and its snapshots: set when `init` asked for `recovery`. */
  let restartable: WasmMainTransport | null = null;
  let keeper: SnapshotKeeper | null = null;
  let schema = 0n;
  let seq = 0;
  let batching = false;
  let batch: ArrayBuffer[] = [];
  let flushQueued = false;

  const flushEnvelopes = (): void => {
    flushQueued = false;
    if (batch.length === 0) return;
    const data = batch;
    batch = [];
    const message: WorkerToHost = { t: "envelopes", data };
    scope.postMessage(message, data);
  };

  // Control messages keep their place behind the envelopes produced before them.
  const post = (message: WorkerToHost, transfer?: Transferable[]): void => {
    flushEnvelopes();
    scope.postMessage(message, transfer);
  };

  const postEnvelope = (kind: Kind, payload: Uint8Array): void => {
    const bytes = encodeEnvelope(kind, seq++ >>> 0, schema, payload);
    const buffer = bytes.buffer as ArrayBuffer;
    if (!batching) {
      post({ t: "envelope", data: buffer }, [buffer]);
      return;
    }
    batch.push(buffer);
    if (!flushQueued) {
      flushQueued = true;
      queueMicrotask(flushEnvelopes);
    }
  };

  /** The ports the host serves asynchronously (protocol 3), or `null` for an older host: every port but the built-ins crosses. */
  let hostPorts: ReadonlySet<number> | null = null;
  /** The ports implemented in the worker (the module of `LoadOptions.worker.ports`). */
  let workerPorts = new Map<number, PortImpl>();

  const crossesToHost = (portId: number): boolean => (hostPorts === null ? !BUILT_IN_PORTS.has(portId) : hostPorts.has(portId));

  const handler: TransportHandler = {
    reply: (payload) => {
      postEnvelope(Kind.Reply, payload);
    },
    changeSet: (payload) => {
      postEnvelope(Kind.ChangeSet, payload);
      keeper?.changed();
    },
    streamItem: (payload) => {
      postEnvelope(Kind.StreamItem, payload);
    },
    portCall: (call) => {
      // A port implemented here: answered in the worker, synchronously when the implementation is.
      const local = workerPorts.get(call.portId);
      if (local !== undefined) {
        const answering = transport;
        return dispatchPortCall(local, call, {
          later: (reply) => {
            // The reply of the instance that asked: not one that replaced it after a trap.
            if (transport !== answering || answering === null) return;
            try {
              answering.send(Kind.PortReply, reply);
            } catch (error) {
              handler.log(4, "undra::worker", `could not deliver a port reply: ${errorMessage(error)}`);
            }
          },
          untyped: (failed, error) => {
            handler.log(4, "undra::worker", `${portOperation(failed, local)} failed: ${errorMessage(error)}`);
          },
        });
      }
      // A port the host serves asynchronously: the main thread runs it and replies later.
      if (crossesToHost(call.portId)) {
        postEnvelope(Kind.PortCall, encodePortCall(call));
        return { kind: "async" };
      }
      // Nobody here or there: "unavailable" makes the wasm shell answer Clock, Rng and Log from its built-in
      // bindings (the worker's own clock, `crypto` and `log` import); any other port is unavailable to the core.
      return { kind: "unavailable" };
    },
    log: (level, target, message) => {
      postEnvelope(Kind.Log, encodeLog({ level, target, message }));
    },
    closed: (error) => {
      post({ t: "closed", failure: toFailure(error) });
    },
  };

  /** A `Call` the core refused has no reply coming; the host is owed one. */
  const answerRefused = (payload: Uint8Array, error: UndraReplyError): void => {
    try {
      const { callId } = decodeCall(payload);
      postEnvelope(Kind.Reply, encodeReply({ callId, status: ReplyStatus.BadRequest, reason: error.reason ?? error.message }));
    } catch {
      // No call id, so nobody is waiting for it.
    }
  };

  const start = async (init: Extract<HostToWorker, { t: "init" }>): Promise<void> => {
    const protocol = init.protocol ?? 1;
    batching = protocol >= 2;
    hostPorts = protocol >= 3 ? new Set(init.asyncPorts ?? []) : null;
    // From the first envelope on: the core talks while it initialises (a log record, the port call of an
    // init hook that reads the cache), before `ready`, and the host rejects an envelope of another schema.
    schema = init.expectedSchemaHash;
    try {
      let adapters: NonNullable<WorkerPortsModule["adapters"]> = {};
      if (init.portsModule !== undefined) {
        const loaded = await loadPortsModule(init.portsModule);
        workerPorts = loaded.ports;
        adapters = loaded.adapters;
      }
      // The core's random source in the worker: WebCrypto, unless the ports module brings its own (ADR-049).
      if (adapters.rng === undefined && !workerPorts.has(PortIds.Rng.portId) && !hasCryptoRandom()) {
        throw new UndraTransportError("unsupported", WEB_CRYPTO_REQUIRED);
      }
      const wasm = new WasmMainTransport({
        wasm: wasmSource(init.wasm),
        expectedSchemaHash: init.expectedSchemaHash,
        platform: init.platform,
        devtools: init.devtools,
        logLevel: init.logLevel,
        ...(adapters.clock !== undefined && { clock: adapters.clock }),
        ...(adapters.rng !== undefined && { rng: adapters.rng }),
        ...(adapters.timer !== undefined && { timer: adapters.timer }),
        onError: (error) => {
          handler.log(4, "undra::worker", `internal error: ${errorMessage(error)}`);
        },
      });
      if (init.recovery !== undefined) {
        // The worker keeps the snapshots (ADR-049 decision 3.3): after the core changed a store, at most one per period.
        restartable = wasm;
        keeper = keeperOf(init.recovery, wasm, (level, target, message) => {
          handler.log(level, target, message);
        });
      }
      const hello = await wasm.start(handler);
      transport = wasm;
      post({ t: "ready", hello, features: WORKER_FEATURES });
    } catch (error) {
      post({ t: "failed", failure: toFailure(error) });
    }
  };

  const relay = (data: ArrayBuffer): void => {
    if (transport === null) return;
    try {
      const envelope = decodeEnvelope(new Uint8Array(data), schema);
      try {
        transport.send(envelope.kind, envelope.payload);
      } catch (error) {
        if (error instanceof UndraReplyError && envelope.kind === Kind.Call) answerRefused(envelope.payload, error);
        else if (!(error instanceof UndraError)) throw error;
        else handler.log(4, "undra::worker", error.message);
      }
    } catch (error) {
      const detail = error instanceof WireError ? error.message : errorMessage(error);
      handler.log(4, "undra::worker", `dropped a malformed envelope from the host: ${detail}`);
    }
  };

  const onMessage = (event: Event): void => {
    const message = (event as MessageEvent).data as HostToWorker;
    switch (message.t) {
      case "init":
        void start(message);
        break;
      case "envelope":
        relay(message.data);
        break;
      case "ports":
        hostPorts = new Set(message.asyncPorts);
        break;
      case "stats": {
        const stats = transport?.stats?.() ?? Promise.resolve(null);
        stats.then(
          (json) => {
            post({ t: "stats", id: message.id, json });
          },
          () => {
            post({ t: "stats", id: message.id, json: null });
          },
        );
        break;
      }
      case "snapshot":
        void snapshot(message.id);
        break;
      case "restore":
        void restore(message.id, message.data);
        break;
      case "restart":
        void restart(message.id, message.generationFloor);
        break;
      case "close":
        transport?.close();
        transport = null;
        restartable = null;
        keeper?.stop();
        keeper = null;
        break;
    }
  };

  /** Answers a `restart` request (ADR-049): the same module again, the kept snapshot restored, after the envelopes it produced. */
  const restart = async (id: number, generationFloor: number): Promise<void> => {
    if (restartable === null || keeper === null) {
      post({ t: "restarted", id, failure: toFailure(new UndraTransportError("unsupported", "the worker was not started with recovery")) });
      return;
    }
    try {
      const result = await restartHere(restartable, keeper, generationFloor, (level, target, message) => {
        handler.log(level, target, message);
      });
      post({ t: "restarted", id, hello: result.hello, restoredFromAgeMs: result.restoredFromAgeMs, storeHandles: result.storeHandles === null ? null : [...result.storeHandles] });
    } catch (error) {
      post({ t: "restarted", id, failure: toFailure(error) });
    }
  };

  /** Answers a `snapshot` request: the bytes (transferred) or why there are none. */
  const snapshot = async (id: number): Promise<void> => {
    try {
      const bytes = await (transport?.snapshot?.() ?? Promise.reject(closedFailure(transport)));
      // `bytes` is a fresh copy whose buffer holds exactly it; anything else is copied before it is transferred.
      const buffer =
        bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength
          ? (bytes.buffer as ArrayBuffer)
          : (bytes.slice().buffer as ArrayBuffer);
      post({ t: "snapshot", id, data: buffer }, [buffer]);
    } catch (error) {
      post({ t: "snapshot", id, failure: toFailure(error) });
    }
  };

  /** Answers a `restore` request after the envelopes it produced (`post` flushes them first). */
  const restore = async (id: number, data: ArrayBuffer): Promise<void> => {
    try {
      await (transport?.restore?.(new Uint8Array(data)) ?? Promise.reject(closedFailure(transport)));
      post({ t: "restored", id, code: 0 });
    } catch (error) {
      if (error instanceof UndraRestoreError) post({ t: "restored", id, code: error.code });
      else post({ t: "restored", id, code: 0, failure: toFailure(error) });
    }
  };

  scope.addEventListener("message", onMessage);
  return () => {
    scope.removeEventListener("message", onMessage);
    transport?.close();
    transport = null;
  };
}

/** Whether `scope` is a dedicated worker's global scope (module workers have no `window`). */
function isWorkerScope(scope: unknown): boolean {
  const ctor = (scope as { WorkerGlobalScope?: new () => unknown }).WorkerGlobalScope;
  return typeof ctor === "function" && scope instanceof ctor;
}

// Loaded as the entry of a Worker: start serving. Imported anywhere else, nothing happens.
if (isWorkerScope(globalThis)) runWorker(globalThis as unknown as WorkerScope);
