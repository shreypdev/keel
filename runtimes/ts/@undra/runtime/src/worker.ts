import { PortIds } from "./adapters/ids.js";
import { UndraError, UndraReplyError, UndraRestoreError, UndraSchemaMismatchError, UndraTransportError } from "./errors.js";
import { errorMessage } from "./platform.js";
import type { Transport, TransportHandler } from "./transport/transport.js";
import { WasmMainTransport, type WasmSource } from "./transport/wasm-main.js";
import {
  type HostToWorker,
  WORKER_FEATURES,
  WORKER_PROTOCOL_VERSION,
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
 * Ports. The core calls `Clock`, `Rng` and `Log` synchronously, while it runs, and cannot wait for
 * the main thread, so the worker answers them itself: it tells the core's built-in bindings to
 * take them ("unavailable" from `port_call`, SPEC 7), which read the worker's own `Date.now`,
 * `crypto.getRandomValues` and `log` import (and the `log` import is relayed to the main
 * thread's `Log` adapter). Every other port call crosses to the main thread and is answered
 * asynchronously, so a port declared `sync` cannot serve the core's synchronous calls here.
 */

/** The ports whose methods the core calls synchronously and the wasm shell binds itself over the `now_ms`, `random` and `log` imports (SPEC 7, `builtin.rs`). */
const BUILT_IN_PORTS: ReadonlySet<number> = new Set([PortIds.Clock.portId, PortIds.Rng.portId, PortIds.Log.portId]);

/** What the worker needs of its global scope; a `DedicatedWorkerGlobalScope` or one end of a `MessageChannel`. */
export interface WorkerScope extends Pick<EventTarget, "addEventListener" | "removeEventListener"> {
  postMessage(message: unknown, transfer?: Transferable[]): void;
}

function toFailure(error: unknown): WorkerFailure {
  if (error instanceof UndraSchemaMismatchError) {
    return { kind: "schemaMismatch", expected: error.expected, got: error.got };
  }
  if (error instanceof UndraTransportError) {
    return { kind: "transport", reason: error.reason, message: error.message };
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

  const handler: TransportHandler = {
    reply: (payload) => {
      postEnvelope(Kind.Reply, payload);
    },
    changeSet: (payload) => {
      postEnvelope(Kind.ChangeSet, payload);
    },
    streamItem: (payload) => {
      postEnvelope(Kind.StreamItem, payload);
    },
    portCall: (call) => {
      // The core calls these three synchronously and cannot wait for the main thread: "unavailable"
      // makes the wasm shell answer from its built-in bindings (the worker's own clock, `crypto`
      // and `log` import). Answering "async" for them would leave `port_call_sync` without an
      // answer, and the core would panic (a trap) on its first clock read.
      if (BUILT_IN_PORTS.has(call.portId)) return { kind: "unavailable" };
      // Everything else is asynchronous here: the main thread runs the port and replies later.
      postEnvelope(Kind.PortCall, encodePortCall(call));
      return { kind: "async" };
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
    batching = (init.protocol ?? 1) >= WORKER_PROTOCOL_VERSION;
    // From the first envelope on: the core talks while it initialises (a log record, the port call of an
    // init hook that reads the cache), before `ready`, and the host rejects an envelope of another schema.
    schema = init.expectedSchemaHash;
    try {
      const wasm = new WasmMainTransport({
        wasm: wasmSource(init.wasm),
        expectedSchemaHash: init.expectedSchemaHash,
        platform: init.platform,
        devtools: init.devtools,
        logLevel: init.logLevel,
        onError: (error) => {
          handler.log(4, "undra::worker", `internal error: ${errorMessage(error)}`);
        },
      });
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
        void restore(message.id, message.bytes);
        break;
      case "close":
        transport?.close();
        transport = null;
        break;
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
      post({ t: "snapshot", id, bytes: buffer }, [buffer]);
    } catch (error) {
      post({ t: "snapshot", id, failure: toFailure(error) });
    }
  };

  /** Answers a `restore` request after the envelopes it produced (`post` flushes them first). */
  const restore = async (id: number, bytes: ArrayBuffer): Promise<void> => {
    try {
      await (transport?.restore?.(new Uint8Array(bytes)) ?? Promise.reject(closedFailure(transport)));
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
