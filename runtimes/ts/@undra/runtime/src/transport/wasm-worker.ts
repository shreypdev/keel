import { UndraError, UndraRestoreError, UndraSchemaMismatchError, UndraTransportError } from "../errors.js";
import { errorMessage, hostPlatform } from "../platform.js";
import {
  type HelloPayload,
  Kind,
  WireError,
  decodeEnvelope,
  decodeLog,
  PortStatus,
  decodePortCall,
  encodeEnvelope,
  encodePortReply,
} from "../wire/index.js";
import type { RestartResult, SnapshotPolicy } from "../recovery.js";
import type { PortOutcome, Transport, TransportHandler } from "./transport.js";
import type { WasmSource } from "./wasm-main.js";
import { type HostToWorker, WORKER_PROTOCOL_VERSION, type WorkerFailure, type WorkerToHost, type WorkerWasm } from "./worker-protocol.js";

/**
 * The part of a `Worker` this transport uses; a real `Worker` fits, and so
 * does one end of a `MessageChannel` (see `terminate` and `close`).
 */
export interface WorkerLike extends Pick<EventTarget, "addEventListener" | "removeEventListener"> {
  postMessage(message: unknown, transfer?: Transferable[]): void;
  /** `Worker`: stops the worker. */
  terminate?(): void;
  /** `MessagePort`: closes the port. */
  close?(): void;
}

/** Options of {@link WasmWorkerTransport}. */
export interface WasmWorkerOptions {
  /** The core's `.wasm`; it is loaded inside the worker. A `BufferSource` is copied, never detached. */
  readonly wasm: WasmSource;
  /** The schema hash the bindings were generated from. */
  readonly expectedSchemaHash: bigint;
  /**
   * The worker to use, or a function that makes it. Default: a module worker
   * on `@undra/runtime/worker` (`new Worker(new URL("../worker.js", import.meta.url), { type: "module" })`,
   * a form bundlers recognise).
   */
  readonly worker?: WorkerLike | (() => WorkerLike);
  /** Platform name for the core; default `"web"` or `"node"`. */
  readonly platform?: string;
  /** Runs the core in `"dev"` mode. */
  readonly devtools?: boolean;
  /** Core log level threshold (0 trace .. 5 fatal). Default 2. */
  readonly logLevel?: number;
  /** How long the worker may take to load and initialise the core, in ms. Default 15000; 0 waits forever. */
  readonly startTimeoutMs?: number;
  /**
   * The module the worker imports before `undra_init` (ADR-049): its default export maps port ids to
   * implementations that run in the worker, which is where a synchronous port (an app's
   * `#[undra::port(sync)]` port, an override of Clock or Rng) must live; see `WorkerPortsModule`.
   */
  readonly ports?: URL | string;
  /**
   * Keep snapshots in the worker for a restart after a trap (`LoadOptions.recovery`, ADR-049). With it, a trap does
   * not end the worker: `restart` can bring the core back.
   */
  readonly recovery?: SnapshotPolicy;
}

/** What waits for the worker's answer to a `snapshot`, `restore` or `restart` request. */
type ControlWaiter =
  | { readonly kind: "snapshot"; resolve(bytes: Uint8Array): void; reject(error: unknown): void }
  | { readonly kind: "restore"; resolve(): void; reject(error: unknown): void }
  | { readonly kind: "restart"; resolve(result: RestartResult): void; reject(error: unknown): void };

/** A transport failure from the worker; a trap keeps the engine's stack as the stack of its `cause` (for the panic report). */
function transportError(failure: Extract<WorkerFailure, { kind: "transport" }>): UndraTransportError {
  if (failure.stack === undefined) return new UndraTransportError(failure.reason, failure.message);
  const cause = new Error(failure.message);
  cause.name = "RuntimeError";
  cause.stack = failure.stack;
  return new UndraTransportError(failure.reason, failure.message, { cause });
}

function failureToError(failure: WorkerFailure): Error {
  switch (failure.kind) {
    case "schemaMismatch":
      return new UndraSchemaMismatchError(failure.expected, failure.got);
    case "transport":
      return transportError(failure);
    case "error":
      return new UndraTransportError("handshake", failure.message);
  }
}

/** Copies the module's bytes so the transfer does not detach the caller's buffer. */
function toWorkerWasm(source: WasmSource): { readonly wasm: WorkerWasm; readonly transfer: Transferable[] } {
  if (source instanceof URL) return { wasm: { kind: "url", href: source.href }, transfer: [] };
  if (source instanceof WebAssembly.Module) return { wasm: { kind: "module", module: source }, transfer: [] };
  const bytes = ArrayBuffer.isView(source)
    ? source.buffer.slice(source.byteOffset, source.byteOffset + source.byteLength)
    : source.slice(0);
  return { wasm: { kind: "bytes", bytes: bytes as ArrayBuffer }, transfer: [bytes as ArrayBuffer] };
}

/** Whether `value` is an `ArrayBuffer`, also one from another realm (a message that crossed a frame or a `vm` context). */
function isArrayBuffer(value: unknown): value is ArrayBuffer {
  return value instanceof ArrayBuffer || Object.prototype.toString.call(value) === "[object ArrayBuffer]";
}

/**
 * The failure of a snapshot or restore request in a form a caller can use: a transport failure
 * keeps its reason, anything else is a protocol failure of this exchange.
 */
function controlFailure(failure: WorkerFailure): Error {
  switch (failure.kind) {
    case "transport":
      return transportError(failure);
    case "schemaMismatch":
      return new UndraSchemaMismatchError(failure.expected, failure.got);
    case "error":
      return new UndraTransportError("protocol", failure.message);
  }
}

/**
 * The `wasm-worker` mode, main-thread half: the core runs in a Worker (see
 * `@undra/runtime/worker`), so heavy calls never block the UI thread. Envelopes
 * travel by `postMessage` with their buffers transferred, the worker's in one
 * message per worker task (so a burst of change-sets costs this thread one
 * task, not one each).
 *
 * Everything this thread does is asynchronous: there is no `callSync`, and the
 * core, which cannot wait for this thread, answers its ports where it runs
 * (ADR-049, worker protocol 3):
 *
 * * `Clock`, `Rng` and `Log` are answered inside the worker by the core's
 *   built-in bindings (the worker's own `Date.now`, `crypto.getRandomValues` and
 *   `log` import). The core's log records still reach `adapters.log`: the worker
 *   relays them. To override Clock, Rng or the core's timers, export them from
 *   the module of {@link WasmWorkerOptions.ports}; `adapters.clock`, `adapters.rng`
 *   and `adapters.timer` of this thread do not reach the worker.
 * * A synchronous port of the app (`#[undra::port(sync)]`) is implemented in the
 *   worker, in that same module, and answered there synchronously. Registering a
 *   synchronous port on this thread is refused (a load-time error, or a throw of
 *   `registerPort` after load) with a message that names it.
 * * Every port registered on this thread with asynchronous methods (Http, Kv,
 *   SecureStore, Fs, an app's async port) is called here, as in `wasm-main`: the
 *   worker knows their ids (`init`, then a `ports` message for each
 *   `registerPort` after load) and forwards their calls.
 *
 * `snapshot()` and `restore()` travel as control messages and are answered in
 * the order the worker handles them, behind the messages sent before them; a
 * restore resolves after the change-sets it produced have been delivered to the
 * handler (see `worker-protocol.ts`).
 */
export class WasmWorkerTransport implements Transport {
  readonly mode = "wasm-worker";
  readonly synchronous = false;
  /** The core runs in the worker and cannot wait for this thread: a synchronous port cannot be served from here (ADR-049). */
  readonly answersSyncPorts = false;

  readonly #options: WasmWorkerOptions;
  #worker: WorkerLike | null = null;
  #handler: TransportHandler | null = null;
  #open = false;
  #closed = false;
  #seq = 0;
  #nextStatsId = 1;
  readonly #stats = new Map<number, { resolve(json: string | null): void; reject(error: unknown): void }>();
  #nextControlId = 1;
  readonly #control = new Map<number, ControlWaiter>();
  /** Whether the worker said, in `ready`, that it understands `snapshot` and `restore`. */
  #canSnapshot = false;
  /** Whether the worker said, in `ready`, that it can restart the core after a trap (and `recovery` is on). */
  #canRestart = false;
  /** The trap the worker reported, while the core waits for `restart` (recovery only). */
  #trapped: UndraTransportError | null = null;
  /** Fails a `start` that has not settled yet (a protocol failure before `ready` must not wait for the timeout). */
  #abortStart: ((error: Error) => void) | null = null;
  #detach: (() => void) | null = null;

  /** @param options See {@link WasmWorkerOptions}. */
  constructor(options: WasmWorkerOptions) {
    this.#options = options;
  }

  start(handler: TransportHandler): Promise<HelloPayload> {
    this.#handler = handler;
    return new Promise<HelloPayload>((resolve, reject) => {
      let worker: WorkerLike;
      try {
        worker = this.#createWorker();
      } catch (error) {
        reject(error);
        return;
      }
      this.#worker = worker;
      const timeoutMs = this.#options.startTimeoutMs ?? 15_000;
      let timer: ReturnType<typeof setTimeout> | undefined;
      let settled = false;
      const settle = (outcome: () => void): void => {
        if (settled) return;
        settled = true;
        if (timer !== undefined) clearTimeout(timer);
        outcome();
      };
      this.#abortStart = (error) => {
        settle(() => {
          this.close();
          reject(error);
        });
      };

      const onMessage = (event: Event): void => {
        const message = (event as MessageEvent).data as WorkerToHost;
        switch (message.t) {
          case "ready":
            if (message.hello.schemaHash !== this.#options.expectedSchemaHash) {
              settle(() => {
                this.close();
                reject(new UndraSchemaMismatchError(this.#options.expectedSchemaHash, message.hello.schemaHash));
              });
              return;
            }
            settle(() => {
              this.#open = true;
              this.#canSnapshot = message.features?.includes("snapshot") === true;
              this.#canRestart = this.#options.recovery !== undefined && message.features?.includes("recovery") === true;
              resolve(message.hello);
            });
            return;
          case "failed": {
            const error = failureToError(message.failure);
            settle(() => {
              this.close();
              reject(error);
            });
            return;
          }
          case "envelope":
            this.#receive(message.data);
            return;
          case "envelopes":
            // One task's worth of the worker's output (protocol 2), in order. A failure stops the rest.
            if (!Array.isArray(message.data)) {
              this.#fail(new UndraTransportError("protocol", "the worker sent an `envelopes` message without a list of envelopes"));
              return;
            }
            for (const data of message.data) {
              if (this.#handler === null) return;
              this.#receive(data);
            }
            return;
          case "stats": {
            const waiting = this.#stats.get(message.id);
            this.#stats.delete(message.id);
            waiting?.resolve(message.json);
            return;
          }
          case "snapshot": {
            const waiting = this.#control.get(message.id);
            if (waiting?.kind !== "snapshot") return;
            this.#control.delete(message.id);
            if (message.failure !== undefined) waiting.reject(controlFailure(message.failure));
            else if (isArrayBuffer(message.data)) waiting.resolve(new Uint8Array(message.data));
            else waiting.reject(new UndraTransportError("protocol", "the worker answered a snapshot request without bytes"));
            return;
          }
          case "restored": {
            const waiting = this.#control.get(message.id);
            if (waiting?.kind !== "restore") return;
            this.#control.delete(message.id);
            if (message.failure !== undefined) waiting.reject(controlFailure(message.failure));
            else if (message.code !== 0) waiting.reject(new UndraRestoreError(message.code));
            else waiting.resolve();
            return;
          }
          case "restarted": {
            const waiting = this.#control.get(message.id);
            if (waiting?.kind !== "restart") return;
            this.#control.delete(message.id);
            if (message.failure !== undefined || message.hello === undefined) {
              waiting.reject(message.failure === undefined ? new UndraTransportError("protocol", "the worker answered a restart without a hello") : failureToError(message.failure));
            } else {
              this.#trapped = null;
              waiting.resolve({ hello: message.hello, restoredFromAgeMs: message.restoredFromAgeMs ?? null, storeHandles: message.storeHandles ?? null });
            }
            return;
          }
          case "closed": {
            const error = failureToError(message.failure);
            if (this.#canRestart && error instanceof UndraTransportError && error.reason === "trap" && this.#open) {
              // Recovery: the worker keeps the compiled module and the last snapshot; the core decides to `restart` or `close`.
              this.#trapped = error;
              this.#handler?.closed(error);
              return;
            }
            this.#fail(error);
            return;
          }
        }
      };
      const onError = (event: Event): void => {
        const text = (event as ErrorEvent).message || "the worker failed";
        const error = new UndraTransportError(this.#open ? "closed" : "handshake", `worker error: ${text}`);
        if (settled) this.#fail(error);
        else
          settle(() => {
            this.close();
            reject(error);
          });
      };
      const onMessageError = (): void => {
        this.#fail(new UndraTransportError("protocol", "the worker sent a message that could not be deserialised"));
      };
      worker.addEventListener("message", onMessage);
      worker.addEventListener("error", onError);
      worker.addEventListener("messageerror", onMessageError);
      this.#detach = () => {
        worker.removeEventListener("message", onMessage);
        worker.removeEventListener("error", onError);
        worker.removeEventListener("messageerror", onMessageError);
      };
      if (timeoutMs > 0) {
        timer = setTimeout(() => {
          settle(() => {
            this.close();
            reject(new UndraTransportError("timeout", `the worker did not start within ${timeoutMs} ms`));
          });
        }, timeoutMs);
      }

      const { wasm, transfer } = toWorkerWasm(this.#options.wasm);
      const init: HostToWorker = {
        t: "init",
        wasm,
        expectedSchemaHash: this.#options.expectedSchemaHash,
        platform: this.#options.platform ?? hostPlatform(),
        devtools: this.#options.devtools === true,
        logLevel: this.#options.logLevel ?? 2,
        protocol: WORKER_PROTOCOL_VERSION,
        asyncPorts: [...(handler.asyncPorts?.() ?? [])],
        ...(this.#options.ports !== undefined && { portsModule: String(this.#options.ports) }),
        ...(this.#options.recovery !== undefined && { recovery: this.#options.recovery }),
      };
      try {
        worker.postMessage(init, transfer);
      } catch (error) {
        settle(() => {
          this.close();
          reject(new UndraTransportError("handshake", `could not reach the worker: ${errorMessage(error)}`, { cause: error }));
        });
      }
    });
  }

  send(kind: Kind, payload: Uint8Array): void {
    // A `PortReply` may go out before `ready`: the core talks while it initialises (an init hook that reads the
    // cache calls the Kv port), and the host's answer to that call is not a message of its own making.
    const starting = !this.#open && kind === Kind.PortReply;
    if ((!this.#open && !starting) || this.#worker === null) {
      throw new UndraTransportError("closed", this.#closed ? "the core is closed" : "the core is not started");
    }
    if (this.#trapped !== null) throw this.#trapped;
    this.#post(kind, payload);
  }

  /**
   * After a trap the worker reported (`recovery`, ADR-049): the worker instantiates the same compiled module again
   * and restores the last snapshot it kept, with its generation floor raised to `generationFloor`. Rejects with an
   * `UndraTransportError` (`"trap"`, with the engine's stack, when the new instance traps too).
   */
  restart(generationFloor: number): Promise<RestartResult> {
    return new Promise<RestartResult>((resolve, reject) => {
      const worker = this.#worker;
      if (!this.#open || worker === null) {
        reject(new UndraTransportError("closed", this.#closed ? "the core is closed" : "the core is not started"));
        return;
      }
      if (!this.#canRestart) {
        reject(new UndraTransportError("unsupported", "the worker was not started with recovery, or its script cannot restart the core"));
        return;
      }
      const id = this.#nextControlId++;
      this.#control.set(id, { kind: "restart", resolve, reject });
      const message: HostToWorker = { t: "restart", id, generationFloor: generationFloor >>> 0 };
      this.#sendControl(id, worker, message);
    });
  }

  /** Tells the worker which ports of this thread it forwards calls to (a `registerPort` after load). */
  portsChanged(asyncPorts: readonly number[]): void {
    const worker = this.#worker;
    if (!this.#open || worker === null) return;
    const message: HostToWorker = { t: "ports", asyncPorts: [...asyncPorts] };
    try {
      worker.postMessage(message);
    } catch {
      // The worker is gone; its loss is reported on its own.
    }
  }

  stats(): Promise<string | null> {
    const worker = this.#worker;
    if (!this.#open || worker === null) return Promise.reject(new UndraTransportError("closed", "the core is closed"));
    return new Promise<string | null>((resolve, reject) => {
      const id = this.#nextStatsId++;
      this.#stats.set(id, { resolve, reject });
      const message: HostToWorker = { t: "stats", id };
      worker.postMessage(message);
    });
  }

  snapshot(): Promise<Uint8Array> {
    return new Promise<Uint8Array>((resolve, reject) => {
      const worker = this.#controlWorker("snapshot");
      const id = this.#nextControlId++;
      this.#control.set(id, { kind: "snapshot", resolve, reject });
      const message: HostToWorker = { t: "snapshot", id };
      this.#sendControl(id, worker, message);
    });
  }

  restore(bytes: Uint8Array): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const worker = this.#controlWorker("restore");
      const id = this.#nextControlId++;
      this.#control.set(id, { kind: "restore", resolve, reject });
      // A private copy is transferred: the caller keeps its bytes.
      const copy = bytes.slice().buffer as ArrayBuffer;
      const message: HostToWorker = { t: "restore", id, data: copy };
      this.#sendControl(id, worker, message, [copy]);
    });
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#open = false;
    this.#handler = null;
    this.#abortStart = null;
    this.#detach?.();
    const worker = this.#worker;
    this.#worker = null;
    for (const waiting of this.#stats.values()) waiting.reject(new UndraTransportError("closed", "the core is closed"));
    this.#stats.clear();
    for (const waiting of this.#control.values()) waiting.reject(new UndraTransportError("closed", "the core is closed"));
    this.#control.clear();
    if (worker === null) return;
    try {
      const message: HostToWorker = { t: "close" };
      worker.postMessage(message);
    } catch {
      // The worker is already gone.
    }
    if (typeof worker.terminate === "function") worker.terminate();
    else worker.close?.();
  }

  /** The worker a snapshot or restore request goes to; throws when the core is closed or the worker script cannot serve it. */
  #controlWorker(operation: "snapshot" | "restore"): WorkerLike {
    if (!this.#open || this.#worker === null) {
      throw new UndraTransportError("closed", this.#closed ? "the core is closed" : "the core is not started");
    }
    if (!this.#canSnapshot) {
      throw new UndraTransportError(
        "unsupported",
        `the worker script does not support ${operation}: it was built before snapshots and restores existed; rebuild it from the same @undra/runtime as the host`,
      );
    }
    return this.#worker;
  }

  /** Posts a control request; a worker that cannot be reached fails it at once. */
  #sendControl(id: number, worker: WorkerLike, message: HostToWorker, transfer: Transferable[] = []): void {
    try {
      worker.postMessage(message, transfer);
    } catch (error) {
      const waiting = this.#control.get(id);
      this.#control.delete(id);
      waiting?.reject(new UndraTransportError("closed", `could not reach the worker: ${errorMessage(error)}`, { cause: error }));
    }
  }

  #createWorker(): WorkerLike {
    const option = this.#options.worker;
    if (typeof option === "function") return option();
    if (option !== undefined) return option;
    if (typeof Worker !== "function") {
      throw new UndraTransportError("unsupported", "this platform has no Worker; pass `worker` to run the core on one");
    }
    return new Worker(new URL("../worker.js", import.meta.url), { type: "module" });
  }

  #post(kind: Kind, payload: Uint8Array): void {
    const bytes = encodeEnvelope(kind, this.#seq++ >>> 0, this.#options.expectedSchemaHash, payload);
    const message: HostToWorker = { t: "envelope", data: bytes.buffer as ArrayBuffer };
    (this.#worker as WorkerLike).postMessage(message, [bytes.buffer as ArrayBuffer]);
  }

  /** The worker or the core died: tell the handler once. */
  #fail(error: Error): void {
    const handler = this.#handler;
    if (this.#closed || handler === null) return;
    if (!this.#open && this.#abortStart !== null) {
      // Still starting: the failure is the start's.
      this.#abortStart(error);
      return;
    }
    this.close();
    handler.closed(error);
  }

  #receive(data: ArrayBuffer): void {
    const handler = this.#handler;
    if (handler === null) return;
    try {
      const envelope = decodeEnvelope(new Uint8Array(data), this.#options.expectedSchemaHash);
      switch (envelope.kind) {
        case Kind.Reply:
          handler.reply(envelope.payload);
          return;
        case Kind.ChangeSet:
          handler.changeSet(envelope.payload);
          return;
        case Kind.StreamItem:
          handler.streamItem(envelope.payload);
          return;
        case Kind.Log: {
          const log = decodeLog(envelope.payload);
          handler.log(log.level, log.target, log.message);
          return;
        }
        case Kind.PortCall: {
          const call = decodePortCall(envelope.payload);
          this.#answerPortCall(call.portCallId, handler.portCall(call));
          return;
        }
        default:
          // Snapshot, or a host-bound kind sent the wrong way: nothing to do with it here.
          return;
      }
    } catch (error) {
      if (error instanceof WireError && error.detail.code === "schema_mismatch") {
        this.#fail(new UndraSchemaMismatchError(error.detail.expected, error.detail.got));
      } else if (error instanceof UndraError) {
        this.#fail(error);
      } else {
        this.#fail(new UndraTransportError("protocol", `bad message from the worker: ${errorMessage(error)}`, { cause: error }));
      }
    }
  }

  /** Sends the answer of a port of this thread: an inline reply now (an asynchronous port that answered without waiting), "unavailable" now, or later. */
  #answerPortCall(portCallId: number, outcome: PortOutcome): void {
    if (outcome.kind === "sync") {
      this.#post(Kind.PortReply, outcome.reply);
    } else if (outcome.kind === "unavailable") {
      this.#post(Kind.PortReply, encodePortReply({ portCallId, status: PortStatus.Unavailable, body: new Uint8Array(0) }));
    }
  }
}
