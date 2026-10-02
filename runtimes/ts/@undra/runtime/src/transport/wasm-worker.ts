import { WEB_CRYPTO_REQUIRED, hasCryptoRandom } from "../adapters/system.js";
import type { LoadOptions, WorkerModeOptions } from "../core.js";
import { UndraError, UndraRestoreError, UndraSchemaMismatchError, UndraTransportError } from "../errors.js";
import { isTrap } from "../panic.js";
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
import type { PortImpl } from "../port.js";
import { portName } from "../port-dispatch.js";
import type { RestartResult, SnapshotPolicy } from "../recovery.js";
import { framed } from "./framed.js";
import type { CoreTransport, PortOutcome, Transport, TransportHandler } from "./transport.js";
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

/** The worker's answers to the requests of {@link WasmWorkerTransport}: `stats`, `snapshot`, `restored`, `restarted`. */
type ControlAnswer = Extract<WorkerToHost, { readonly t: "stats" | "snapshot" | "restored" | "restarted" }>;

/** What waits for the worker's answer to a request: the answer, as it came. */
interface ControlWaiter {
  resolve(answer: ControlAnswer): void;
  reject(error: unknown): void;
}

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
 * The ids of `ports` the worker forwards to the host's thread. A synchronous port cannot be served from there, because
 * the core cannot wait for that thread: it is refused, naming it and saying where it belongs (ADR-049).
 */
function asyncPortIds(ports: Iterable<readonly [number, PortImpl]> = []): number[] {
  const ids: number[] = [];
  for (const [portId, impl] of ports) {
    if (impl.sync) throw new UndraError("options", syncPortRefusal(portId, impl));
    ids.push(portId);
  }
  return ids;
}

/**
 * The failure an answer of the worker carries, in a form a caller can use: a transport failure keeps its reason (a
 * trap its stack), anything else, and an answer without what it should carry, is a protocol failure of this exchange.
 */
function controlFailure(answer: ControlAnswer): Error {
  const failure = "failure" in answer ? answer.failure : undefined;
  if (failure === undefined) return new UndraTransportError("protocol", "the worker gave an incomplete answer");
  return failure.kind === "error" ? new UndraTransportError("protocol", failure.message) : failureToError(failure);
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

  readonly #options: WasmWorkerOptions;
  #worker: WorkerLike | null = null;
  #handler: TransportHandler | null = null;
  #open = false;
  #closed = false;
  #seq = 0;
  #nextControlId = 1;
  readonly #control = new Map<number, ControlWaiter>();
  /** Whether the worker said, in `ready`, that it understands `snapshot` and `restore`. */
  #canSnapshot = false;
  /** Whether the worker said, in `ready`, that it can restart the core after a trap (and `recovery` is on). */
  #canRestart = false;
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
      let asyncPorts: number[];
      try {
        // A synchronous port of this thread is refused before anything is spawned or sent.
        asyncPorts = asyncPortIds(handler.ports?.());
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
            // Ports answered in the worker need protocol 3 (ADR-049): a worker script older than this runtime would
            // ignore the module and send the app's synchronous ports across, where the core traps at their first call.
            if (this.#options.ports !== undefined && message.features?.includes("ports") !== true) {
              settle(() => {
                this.close();
                reject(new UndraTransportError("unsupported", "worker.ports needs this @undra/runtime's worker script (protocol 3)"));
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
          case "stats":
          case "snapshot":
          case "restored":
          case "restarted": {
            const waiting = this.#control.get(message.id);
            this.#control.delete(message.id);
            waiting?.resolve(message);
            return;
          }
          case "closed": {
            const error = failureToError(message.failure);
            // With recovery the worker keeps the compiled module and the last snapshot: the trap is the handler's to
            // `restart` from (it refuses calls meanwhile), or to `close` on.
            if (this.#canRestart && isTrap(error)) this.#handler?.closed(error);
            else this.#fail(error);
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
        asyncPorts,
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
    this.#post(kind, payload);
  }

  /**
   * After a trap the worker reported (`recovery`, ADR-049): the worker instantiates the same compiled module again
   * and restores the last snapshot it kept, with its generation floor raised to `generationFloor`. Rejects with an
   * `UndraTransportError` (`"trap"`, with the engine's stack, when the new instance traps too).
   */
  async restart(generationFloor: number): Promise<RestartResult> {
    const answer = await this.#request(this.#canRestart, "restart", { t: "restart", id: 0, generationFloor });
    // The answer carries the result's fields (a failure carries no `hello`).
    if (answer.t !== "restarted" || answer.hello === undefined) throw controlFailure(answer);
    return answer as RestartResult;
  }

  /** Refuses a synchronous port; tells a started worker to forward the calls of an asynchronous one here (a `registerPort`). */
  portAdded(portId: number, impl: PortImpl): void {
    const asyncPorts = asyncPortIds([...(this.#handler?.ports?.() ?? []), [portId, impl]]);
    const worker = this.#worker;
    if (!this.#open || worker === null) return;
    const message: HostToWorker = { t: "ports", asyncPorts };
    try {
      worker.postMessage(message);
    } catch {
      // The worker is gone; its loss is reported on its own.
    }
  }

  async stats(): Promise<string | null> {
    const answer = await this.#request(true, "stats", { t: "stats", id: 0 });
    return answer.t === "stats" ? answer.json : null;
  }

  async snapshot(): Promise<Uint8Array> {
    const answer = await this.#request(this.#canSnapshot, "snapshot", { t: "snapshot", id: 0 });
    if (answer.t !== "snapshot" || !isArrayBuffer(answer.data)) throw controlFailure(answer);
    return new Uint8Array(answer.data);
  }

  async restore(bytes: Uint8Array): Promise<void> {
    // A private copy is transferred: the caller keeps its bytes.
    const copy = bytes.slice().buffer as ArrayBuffer;
    const answer = await this.#request(this.#canSnapshot, "restore", { t: "restore", id: 0, data: copy }, [copy]);
    if (answer.t !== "restored" || answer.failure !== undefined) throw controlFailure(answer);
    if (answer.code !== 0) throw new UndraRestoreError(answer.code);
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

  /**
   * Sends a control request (`message` with a fresh `id`) and resolves with the worker's answer to it. Rejects when the
   * core is closed, when the worker script cannot serve `operation` (`can` is false), or when the worker is unreachable.
   */
  #request(can: boolean, operation: string, message: HostToWorker & { readonly id: number }, transfer: Transferable[] = []): Promise<ControlAnswer> {
    return new Promise<ControlAnswer>((resolve, reject) => {
      const worker = this.#worker;
      if (!this.#open || worker === null) throw new UndraTransportError("closed", this.#closed ? "the core is closed" : "the core is not started");
      if (!can) throw new UndraTransportError("unsupported", `the worker script cannot ${operation}: rebuild it with this @undra/runtime`);
      const id = this.#nextControlId++;
      this.#control.set(id, { resolve, reject });
      try {
        worker.postMessage({ ...message, id }, transfer);
      } catch (error) {
        this.#control.delete(id);
        reject(new UndraTransportError("closed", `could not reach the worker: ${errorMessage(error)}`, { cause: error }));
      }
    });
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

/**
 * The text of the error that refuses a synchronous port on a thread the core cannot wait for (`wasm-worker`, ADR-049): the port
 * and the fix (the other fix, mode `"wasm-main"`, is in the docs of `registerPort`). Here, with the transport that raises it.
 */
function syncPortRefusal(portId: number, impl?: Pick<PortImpl, "name">): string {
  return `${portName(portId, impl)} is synchronous: in wasm-worker mode, register it in LoadOptions.worker.ports`;
}

/**
 * The transport `UndraCore.load` makes for `mode: "wasm-worker"` (ADR-057): this module maps the options and makes the checks of
 * its own mode, so a page that never asks for it does not carry them, and hands the core the transport already framed, so the
 * adapter that frames its messages arrives in the same fetch wave as this module. `recovery` is the policy the worker keeps
 * snapshots by (data, not code).
 */
export function workerTransport(options: LoadOptions, recovery: WasmWorkerOptions["recovery"]): CoreTransport {
  if (options.wasm === undefined) throw new UndraError("options", "mode 'wasm-worker' needs the `wasm` option");
  // Checked here, before the worker is spawned; the worker reads its own `crypto` (ADR-049).
  if (!hasCryptoRandom()) throw new UndraTransportError("unsupported", WEB_CRYPTO_REQUIRED);
  // A Worker (anything with `postMessage`) or a function creating one is `{ create }` in short.
  const worker = options.worker;
  const { create, ports } = (typeof worker === "object" && !("postMessage" in worker) ? worker : { create: worker }) as WorkerModeOptions;
  return framed(
    new WasmWorkerTransport({
      wasm: options.wasm,
      expectedSchemaHash: options.expectedSchemaHash,
      ...(create && { worker: create }),
      ...(ports !== undefined && { ports }),
      ...(recovery && { recovery }),
      ...(options.platform !== undefined && { platform: options.platform }),
      ...(options.devtools !== undefined && { devtools: options.devtools }),
      ...(options.logLevel !== undefined && { logLevel: options.logLevel }),
      ...(options.handshakeTimeoutMs !== undefined && { startTimeoutMs: options.handshakeTimeoutMs }),
    }),
  );
}
