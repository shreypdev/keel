import { UndraError, UndraSchemaMismatchError, UndraTransportError } from "../errors.js";
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
}

function failureToError(failure: WorkerFailure): Error {
  switch (failure.kind) {
    case "schemaMismatch":
      return new UndraSchemaMismatchError(failure.expected, failure.got);
    case "transport":
      return new UndraTransportError(failure.reason, failure.message);
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

/**
 * The `wasm-worker` mode, main-thread half: the core runs in a Worker (see
 * `@undra/runtime/worker`), so heavy calls never block the UI thread. Envelopes
 * travel by `postMessage` with their buffers transferred, the worker's in one
 * message per worker task (so a burst of change-sets costs this thread one
 * task, not one each); port calls of the core execute here, on the main
 * thread, and are answered with `PortReply`.
 *
 * Everything is asynchronous: there is no `callSync`, and a port declared
 * `sync` cannot serve the core's synchronous calls (the core cannot block on
 * this thread). `adapters.clock`, `adapters.rng` and `adapters.timer` do not
 * cross to the worker; the core uses its built-in bindings there.
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
  #nextStatsId = 1;
  readonly #stats = new Map<number, { resolve(json: string | null): void; reject(error: unknown): void }>();
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
          case "closed":
            this.#fail(failureToError(message.failure));
            return;
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
    if (!this.#open || this.#worker === null) {
      throw new UndraTransportError("closed", this.#closed ? "the core is closed" : "the core is not started");
    }
    this.#post(kind, payload);
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

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#open = false;
    this.#handler = null;
    this.#detach?.();
    const worker = this.#worker;
    this.#worker = null;
    for (const waiting of this.#stats.values()) waiting.reject(new UndraTransportError("closed", "the core is closed"));
    this.#stats.clear();
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

  #answerPortCall(portCallId: number, outcome: PortOutcome): void {
    if (outcome.kind === "sync") this.#post(Kind.PortReply, outcome.reply);
    else if (outcome.kind === "unavailable") {
      this.#post(Kind.PortReply, encodePortReply({ portCallId, status: PortStatus.Unavailable, body: new Uint8Array(0) }));
    }
  }
}
