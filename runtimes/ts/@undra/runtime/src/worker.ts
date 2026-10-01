import { UndraError, UndraReplyError, UndraSchemaMismatchError, UndraTransportError } from "./errors.js";
import { errorMessage } from "./platform.js";
import type { Transport, TransportHandler } from "./transport/transport.js";
import { WasmMainTransport, type WasmSource } from "./transport/wasm-main.js";
import {
  type HostToWorker,
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
 */

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
      // The core cannot wait for the main thread, so every port call is asynchronous here.
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
      schema = init.expectedSchemaHash;
      post({ t: "ready", hello });
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
      case "close":
        transport?.close();
        transport = null;
        break;
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
