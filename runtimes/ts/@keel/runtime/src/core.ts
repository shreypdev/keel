import { browserAdapters } from "./adapters/browser.js";
import { standardPorts, startEventSources, timerPort } from "./adapters/ports.js";
import { PortIds } from "./adapters/ids.js";
import { consoleLog } from "./adapters/system.js";
import type { Adapters, AdapterOverrides } from "./adapters/types.js";
import {
  KeelError,
  KeelModeError,
  KeelPortError,
  KeelReplyError,
  KeelSchemaMismatchError,
  KeelTransportError,
} from "./errors.js";
import { nextCallId } from "./callid.js";
import { Mirror } from "./mirror.js";
import { errorMessage } from "./platform.js";
import type { PortImpl } from "./port.js";
import { StreamCall } from "./stream.js";
import { RemoteTransport, type WebSocketFactory } from "./transport/remote.js";
import type { PortOutcome, Transport, TransportHandler } from "./transport/transport.js";
import { WasmMainTransport, type WasmSource } from "./transport/wasm-main.js";
import type { WorkerLike } from "./transport/wasm-worker.js";
import {
  CallTarget,
  type Handle,
  type HelloPayload,
  Kind,
  PortStatus,
  ReplyStatus,
  StreamFlag,
  type PortCallPayload,
  codecs,
  decodeValue,
  encodeCall,
  encodeCancel,
  encodeEvent,
  encodeObserve,
  encodePortReply,
  encodeRelease,
  encodeStreamCredit,
  encodeTimerFired,
} from "./wire/index.js";

/** How the core is reached (SPEC 17.1). */
export type LoadMode = "wasm-main" | "wasm-worker" | "remote";

/**
 * What a call addresses: a free function, or a method of the object behind a
 * handle. (Addition to SPEC 17.1: the wire `CallTarget` enum has no room for
 * the handle, so generated code passes this object.) A bare
 * `CallTarget.FreeFunction` is accepted as shorthand for the first form.
 */
export type CallTargetRef =
  | { readonly target: CallTarget.FreeFunction }
  | { readonly target: CallTarget.ObjectMethod; readonly handle: Handle };

/** The argument type of `call`, `callSync` and `stream`. */
export type CallTargetArg = CallTargetRef | CallTarget.FreeFunction;

/** Live counters of a core; see {@link KeelCore.stats}. */
export interface KeelStats {
  /** Object handles alive in the core (its own count when it reports one, else the handles this runtime constructed and has not released). */
  readonly liveHandles: number;
  /** Calls sent and not yet answered. */
  readonly pendingCalls: number;
  /** Streams opened and not yet finished. */
  readonly openStreams: number;
  /** Stores registered with the mirror. */
  readonly mirroredStores: number;
  /** Change-set entries dropped because their store was gone. */
  readonly droppedEntries: number;
  /** The core's own statistics (`keel_stats_json`, parsed) when the transport can ask for them: wasm modes. `null` over a socket. */
  readonly core: Readonly<Record<string, unknown>> | null;
}

/** Options shared by `KeelCore.load` and `KeelCore.attach`. */
export interface AttachOptions {
  /** The schema hash of the generated bindings (`KeelIds.schemaHash`); a core built from another schema is refused. */
  readonly expectedSchemaHash: bigint;
  /**
   * Adapters to use instead of the browser defaults (`browserAdapters()`):
   * a value replaces the default of that port, `null` removes it. `timer`,
   * `clock` and `rng` back the wasm imports (main-thread wasm only); `log`
   * receives the core's log records; `http`, `kv`, `secureStore` and `fs`
   * become ports; `connectivity` and `lifecycle` become event sources.
   */
  readonly adapters?: AdapterOverrides;
  /** More ports to register, by port id, in addition to the adapters (custom ports of your core). */
  readonly ports?: Readonly<Record<number, PortImpl>>;
  /** How long `observe` waits for the initial change-set of a remote or worker core before it rejects, in ms. Default 10000; 0 waits forever. */
  readonly observeTimeoutMs?: number;
  /** Called once when the channel to the core is lost (not when you call `close()`). */
  readonly onClose?: (error: Error) => void;
  /** Called with failures that have no caller to reject: a change-set that did not decode, a store whose `_apply` threw, a port that failed. They are also logged. */
  readonly onError?: (error: unknown) => void;
  /** Make this core `KeelCore.shared` when none is set yet. Default `true`. */
  readonly shared?: boolean;
}

/** Options of `KeelCore.load`. */
export interface LoadOptions extends AttachOptions {
  /** Where the core runs: `"wasm-main"` (this thread), `"wasm-worker"` (a Worker) or `"remote"` (a native core over WebSocket). */
  readonly mode: LoadMode;
  /** The core's `.wasm`, for the wasm modes: a URL to fetch, its bytes, or a compiled `WebAssembly.Module`. */
  readonly wasm?: WasmSource;
  /** `ws://` or `wss://` URL of the core, for `"remote"`. */
  readonly url?: string;
  /** Run the core in `"dev"` mode: it emits devtools log records (SPEC 5.10). */
  readonly devtools?: boolean;
  /** Platform name reported to the core. Default `"web"` (`"node"` under Node.js). */
  readonly platform?: string;
  /** Core log threshold, 0 trace .. 5 fatal, for the wasm modes. Default 2. */
  readonly logLevel?: number;
  /** Handshake timeout for `"remote"` and `"wasm-worker"`, in ms. */
  readonly handshakeTimeoutMs?: number;
  /** WebSocket implementation for `"remote"`; default the global one. */
  readonly webSocket?: WebSocketFactory;
  /** The Worker for `"wasm-worker"`, or a function creating it; default a module worker on `@keel/runtime/worker`. */
  readonly worker?: WorkerLike | (() => WorkerLike);
}

/** Statistics counters exposed by `keel_stats_json` that this runtime reads. */
type CoreStatsJson = Readonly<Record<string, unknown>>;

interface PendingCall {
  readonly kind: "call";
  resolve(body: Uint8Array): void;
  reject(error: unknown): void;
  cleanup: (() => void) | undefined;
}

interface PendingStream {
  readonly kind: "stream";
  readonly stream: StreamCall;
}

const UNAVAILABLE: PortOutcome = { kind: "unavailable" };
const ASYNC: PortOutcome = { kind: "async" };
const NO_BYTES = new Uint8Array(0);
const DEFAULT_OBSERVE_TIMEOUT_MS = 10_000;

function isThenable(value: unknown): value is PromiseLike<Uint8Array> {
  return typeof value === "object" && value !== null && typeof (value as { then?: unknown }).then === "function";
}

function abortReason(signal: AbortSignal): unknown {
  if (signal.reason !== undefined) return signal.reason;
  const error = new Error("The operation was aborted");
  error.name = "AbortError";
  return error;
}

function encodeTarget(target: CallTargetArg, methodId: number, callId: number, args: Uint8Array): Uint8Array {
  if (typeof target === "number") {
    if (target !== CallTarget.FreeFunction) {
      throw new TypeError("a bare CallTarget must be FreeFunction; pass { target, handle } for a method");
    }
    return encodeCall({ target: CallTarget.FreeFunction, methodId, callId, args });
  }
  if (target.target === CallTarget.ObjectMethod) {
    return encodeCall({ target: CallTarget.ObjectMethod, handle: target.handle, methodId, callId, args });
  }
  return encodeCall({ target: CallTarget.FreeFunction, methodId, callId, args });
}

/** Overlays `overrides` on `base`: a value replaces, `null` removes. */
function mergeAdapters(base: Partial<Adapters>, overrides: AdapterOverrides | undefined): Partial<Adapters> {
  const merged: Record<string, unknown> = { ...base };
  if (overrides !== undefined) {
    for (const [name, value] of Object.entries(overrides)) {
      if (value === null) delete merged[name];
      else if (value !== undefined) merged[name] = value;
    }
  }
  return merged as Partial<Adapters>;
}

/**
 * The host side of a Keel core (docs/SPEC.md sections 11 and 17.1): owns the
 * transport, allocates call ids and routes replies to the promises and streams
 * that wait for them, holds the {@link Mirror} that applies change-sets, runs
 * the host's port implementations, and enforces the schema-hash check.
 *
 * ```ts
 * const core = await KeelCore.load({
 *   mode: "wasm-main",
 *   wasm: new URL("./todo_core.wasm", import.meta.url),
 *   expectedSchemaHash: KeelIds.schemaHash,
 * });
 * const todos = await Todos.create();   // uses KeelCore.shared
 * ```
 *
 * Generated code calls `call`, `callSync`, `stream`, `construct`, `observe`,
 * `release`, `event`, `registerPort` and reads `mirror` and `shared`; the rest
 * is for applications and tests.
 */
export class KeelCore {
  static #shared: KeelCore | null = null;

  /** The core that the generated constructors and functions default to: the first one loaded. Throws `KeelError` (`"state"`) when there is none. */
  static get shared(): KeelCore {
    if (KeelCore.#shared === null) {
      throw new KeelError("state", "no KeelCore is loaded; call KeelCore.load(...) first, or pass a core explicitly");
    }
    return KeelCore.#shared;
  }

  /**
   * Loads a core: creates the transport of `options.mode`, performs the
   * handshake and resolves with the running core. Rejects with
   * `KeelSchemaMismatchError` when the core was built from another schema than
   * `expectedSchemaHash`, and with `KeelTransportError` when it cannot be
   * reached or started. The first core loaded becomes {@link KeelCore.shared}.
   */
  static async load(options: LoadOptions): Promise<KeelCore> {
    const adapters = mergeAdapters(browserAdapters(), options.adapters);
    let transport: Transport;
    switch (options.mode) {
      case "wasm-main": {
        if (options.wasm === undefined) throw new KeelError("options", "mode 'wasm-main' needs the `wasm` option");
        transport = new WasmMainTransport({
          wasm: options.wasm,
          expectedSchemaHash: options.expectedSchemaHash,
          ...(options.platform !== undefined && { platform: options.platform }),
          ...(options.devtools !== undefined && { devtools: options.devtools }),
          ...(options.logLevel !== undefined && { logLevel: options.logLevel }),
          ...(adapters.clock && { clock: adapters.clock }),
          ...(adapters.rng && { rng: adapters.rng }),
          ...(adapters.timer && { timer: adapters.timer }),
          onError: (error) => {
            adapters.log?.log(4, "keel::runtime", `import failed: ${errorMessage(error)}`);
          },
        });
        break;
      }
      case "wasm-worker": {
        if (options.wasm === undefined) throw new KeelError("options", "mode 'wasm-worker' needs the `wasm` option");
        const { WasmWorkerTransport } = await import("./transport/wasm-worker.js");
        transport = new WasmWorkerTransport({
          wasm: options.wasm,
          expectedSchemaHash: options.expectedSchemaHash,
          ...(options.worker !== undefined && { worker: options.worker }),
          ...(options.platform !== undefined && { platform: options.platform }),
          ...(options.devtools !== undefined && { devtools: options.devtools }),
          ...(options.logLevel !== undefined && { logLevel: options.logLevel }),
          ...(options.handshakeTimeoutMs !== undefined && { startTimeoutMs: options.handshakeTimeoutMs }),
        });
        break;
      }
      case "remote": {
        if (options.url === undefined) throw new KeelError("options", "mode 'remote' needs the `url` option");
        transport = new RemoteTransport({
          url: options.url,
          expectedSchemaHash: options.expectedSchemaHash,
          ...(options.platform !== undefined && { platform: options.platform }),
          ...(options.devtools !== undefined && { devtools: options.devtools }),
          ...(options.webSocket !== undefined && { webSocket: options.webSocket }),
          ...(options.handshakeTimeoutMs !== undefined && { handshakeTimeoutMs: options.handshakeTimeoutMs }),
        });
        break;
      }
      default:
        throw new KeelError("options", `unknown mode '${String((options as { mode: unknown }).mode)}'`);
    }
    return KeelCore.#attach(transport, options, adapters);
  }

  /**
   * Runs a core over a transport you provide (an embedder's IPC channel, a
   * test double) instead of one of the built-in modes. Everything else is as
   * for {@link KeelCore.load}, including the schema check on the transport's
   * `Hello`.
   */
  static attach(transport: Transport, options: AttachOptions): Promise<KeelCore> {
    return KeelCore.#attach(transport, options, mergeAdapters(browserAdapters(), options.adapters));
  }

  static async #attach(transport: Transport, options: AttachOptions, adapters: Partial<Adapters>): Promise<KeelCore> {
    const core = new KeelCore(transport, options, adapters);
    try {
      await core.#start();
    } catch (error) {
      core.#dispose(null);
      throw error;
    }
    if (options.shared !== false && KeelCore.#shared === null) KeelCore.#shared = core;
    return core;
  }

  /** The mirror that applies change-sets to stores; stores register themselves with it. */
  readonly mirror: Mirror;
  /** What the core said in its `Hello` (for wasm modes, synthesised from the module). Set once `load` resolves. */
  hello: HelloPayload = { keelVersion: "", schemaHash: 0n, platform: "", mode: "" };

  readonly #transport: Transport;
  readonly #options: AttachOptions;
  readonly #adapters: Partial<Adapters>;
  readonly #observeTimeoutMs: number;
  readonly #ports = new Map<number, PortImpl>();
  readonly #pending = new Map<number, PendingCall | PendingStream>();
  readonly #handles = new Set<Handle>();
  #nextCallId = 0;
  #closed = false;
  #stopEvents: (() => void) | null = null;

  private constructor(transport: Transport, options: AttachOptions, adapters: Partial<Adapters>) {
    this.#transport = transport;
    this.#options = options;
    this.#adapters = adapters;
    this.#observeTimeoutMs = options.observeTimeoutMs ?? DEFAULT_OBSERVE_TIMEOUT_MS;
    this.mirror = new Mirror({
      onError: (error) => {
        this.#reportError("mirror", error);
      },
    });
    for (const [portId, impl] of standardPorts(adapters)) this.#ports.set(portId, impl);
    if (options.ports !== undefined) {
      for (const [portId, impl] of Object.entries(options.ports)) this.#ports.set(Number(portId), impl);
    }
  }

  /** The mode of the transport (`"wasm-main"`, `"wasm-worker"`, `"remote"`, or a custom one). */
  get mode(): string {
    return this.#transport.mode;
  }

  /** Whether the core has been closed, by `close()` or because the channel was lost. */
  get closed(): boolean {
    return this.#closed;
  }

  // ----- calls -----------------------------------------------------------------------

  /**
   * Calls a synchronous method and returns its result directly. Available in
   * `wasm-main` only; every other mode throws {@link KeelModeError}. Rejects
   * asynchronous methods (the core answers status 5). Throws
   * {@link KeelReplyError} when the call does not succeed.
   */
  callSync(target: CallTargetArg, methodId: number, args: Uint8Array): Uint8Array {
    const transport = this.#transport;
    if (transport.callSync === undefined) throw new KeelModeError("callSync", transport.mode);
    this.#assertOpen();
    const reply = transport.callSync(encodeTarget(target, methodId, this.#allocCallId(), args));
    if (reply.length < 5) throw new KeelTransportError("protocol", "the core returned a truncated reply");
    const status = reply[4] as number;
    const body = reply.subarray(5);
    if (status === ReplyStatus.Ok) return body;
    throw new KeelReplyError(status as ReplyStatus, body);
  }

  /**
   * Calls a method and resolves with the reply body. Rejects with
   * {@link KeelReplyError} (status 1 typed error, 2 panic, 3 cancelled, 5 bad
   * request) or {@link KeelTransportError}. When `signal` aborts, the call is
   * cancelled in the core (`Cancel`) and the promise rejects at once with the
   * signal's reason; an already aborted signal never sends anything.
   */
  call(target: CallTargetArg, methodId: number, args: Uint8Array, signal?: AbortSignal): Promise<Uint8Array> {
    if (signal?.aborted === true) return Promise.reject(abortReason(signal));
    return this.#request((callId) => encodeTarget(target, methodId, callId, args), signal);
  }

  /**
   * Opens a stream. Each iteration of the returned iterable is a separate
   * call: `for await` opens it, grants the core 16 items of credit, tops the
   * credit up as items are consumed, and closes the stream with `Cancel` when
   * the loop is left early. Item bodies are undecoded; a failure of the
   * stream rejects with {@link KeelReplyError} (status 1 with the encoded
   * error, like a failed call).
   */
  stream(target: CallTargetArg, methodId: number, args: Uint8Array): AsyncIterable<Uint8Array> {
    return { [Symbol.asyncIterator]: () => this.#openStream(target, methodId, args) };
  }

  /**
   * Runs a constructor (`typeId` names the object type, `methodId` the
   * constructor) and resolves with the new object's handle. Rejects like `call`.
   */
  async construct(typeId: number, methodId: number, args: Uint8Array): Promise<Handle> {
    const body = await this.#request((callId) =>
      encodeCall({ target: CallTarget.Constructor, typeId, methodId, callId, args }),
    );
    const handle = decodeValue(codecs.u64, body);
    this.#handles.add(handle);
    return handle;
  }

  /**
   * Starts (`on`) or stops observing a signal of a store (`ALL_SIGNALS` for
   * every one). Starting resolves once the initial values have reached the
   * mirror and been applied to the store's signals: at once for an in-process
   * core (the core delivers them before `observe` returns), when the first
   * change-set entry for the observation has been applied for a worker or
   * remote core. The latter rejects with `KeelError("observe")` after
   * `observeTimeoutMs` if none arrives, which means the handle is not a live
   * store.
   */
  observe(handle: Handle, signalId: number, on: boolean): Promise<void> {
    try {
      this.#assertOpen();
      this.#transport.send(Kind.Observe, encodeObserve({ handle, signalId, on }));
    } catch (error) {
      return Promise.reject(error);
    }
    if (this.#transport.synchronous) {
      // The core has already delivered the initial change-set; apply it now.
      this.mirror.flush();
      return Promise.resolve();
    }
    return on ? this.mirror.whenObserved(handle, signalId, this.#observeTimeoutMs) : Promise.resolve();
  }

  /** Releases an object handle: the store is unregistered from the mirror and the core drops its reference. Unknown handles and a closed core are ignored. */
  release(handle: Handle): void {
    this.mirror.unregister(handle);
    this.#handles.delete(handle);
    if (this.#closed) return;
    try {
      this.#transport.send(Kind.Release, encodeRelease({ handle }));
    } catch (error) {
      this.#reportError("release", error);
    }
  }

  /** Sends a host-to-core event of an event port (`Connectivity.changed`, `Lifecycle.changed`, ...). Throws {@link KeelTransportError} when the core is closed. */
  event(portId: number, methodId: number, payload: Uint8Array): void {
    this.#assertOpen();
    this.#transport.send(Kind.Event, encodeEvent({ portId, methodId, payload }));
  }

  /** Tells the core that a timer it set through a foreign `Timer` port is due (see `timerPort`). Wasm cores own their timers and do not need this. */
  timerFired(timerId: number): void {
    this.#assertOpen();
    this.#transport.send(Kind.TimerFired, encodeTimerFired({ timerId }));
  }

  /**
   * Registers the host implementation of a core port (SPEC 17.1), replacing
   * any earlier one for `portId`. Generated code offers `<name>PortImpl`
   * adapters that build the {@link PortImpl} from a typed implementation.
   */
  registerPort(portId: number, impl: PortImpl): void {
    this.#ports.set(portId, impl);
  }

  /** Live counters of this core; see {@link KeelStats}. */
  async stats(): Promise<KeelStats> {
    let core: CoreStatsJson | null = null;
    const json = this.#closed ? null : await this.#transport.stats?.();
    if (typeof json === "string") {
      try {
        const parsed: unknown = JSON.parse(json);
        if (typeof parsed === "object" && parsed !== null) core = parsed as CoreStatsJson;
      } catch {
        core = null;
      }
    }
    let calls = 0;
    let streams = 0;
    for (const p of this.#pending.values()) {
      if (p.kind === "call") calls++;
      else streams++;
    }
    const coreHandles = core?.live_handles;
    return {
      liveHandles: typeof coreHandles === "number" ? coreHandles : this.#handles.size,
      pendingCalls: calls,
      openStreams: streams,
      mirroredStores: this.mirror.size,
      droppedEntries: this.mirror.dropped,
      core,
    };
  }

  /**
   * Closes the core: in-flight calls and streams reject with
   * {@link KeelTransportError}, event sources stop, the transport is released.
   * Idempotent. Later calls reject or throw.
   */
  close(): void {
    this.#dispose(new KeelTransportError("closed", "the core was closed"));
  }

  // ----- internals -------------------------------------------------------------------

  async #start(): Promise<void> {
    const hello = await this.#transport.start(this.#handler);
    if (hello.schemaHash !== this.#options.expectedSchemaHash) {
      throw new KeelSchemaMismatchError(this.#options.expectedSchemaHash, hello.schemaHash);
    }
    this.hello = hello;
    if (this.#options.adapters?.timer && this.#transport.mode === "remote") {
      // A native core normally times itself; an explicit Timer adapter is a request to serve its Timer port.
      this.#ports.set(
        PortIds.Timer.portId,
        timerPort(this.#options.adapters.timer, (timerId) => {
          try {
            this.timerFired(timerId);
          } catch (error) {
            this.#reportError("timer", error);
          }
        }),
      );
    }
    this.#stopEvents = startEventSources(this, this.#adapters, (error) => {
      this.#reportError("event", error);
    });
  }

  #assertOpen(): void {
    if (this.#closed) throw new KeelTransportError("closed", "the core is closed");
  }

  /** Stops everything. `reason` is what pending work fails with; `null` when there is none (a failed start). */
  #dispose(reason: Error | null): void {
    if (this.#closed) return;
    this.#closed = true;
    if (KeelCore.#shared === this) KeelCore.#shared = null;
    this.#stopEvents?.();
    this.#stopEvents = null;
    const failure = reason ?? new KeelTransportError("closed", "the core is closed");
    const pending = [...this.#pending.values()];
    this.#pending.clear();
    for (const p of pending) {
      if (p.kind === "call") {
        p.cleanup?.();
        p.reject(failure);
      } else {
        p.stream.fail(failure);
      }
    }
    this.mirror.failWaiters(failure);
    this.#transport.close();
  }

  /** The channel to the core was lost. */
  #lost(error: Error): void {
    if (this.#closed) return;
    this.#dispose(error);
    try {
      this.#options.onClose?.(error);
    } catch (thrown) {
      this.#reportError("onClose", thrown);
    }
  }

  #allocCallId(): number {
    this.#nextCallId = nextCallId(this.#nextCallId, (id) => this.#pending.has(id));
    return this.#nextCallId;
  }

  #request(encode: (callId: number) => Uint8Array, signal?: AbortSignal): Promise<Uint8Array> {
    try {
      this.#assertOpen();
    } catch (error) {
      return Promise.reject(error);
    }
    const callId = this.#allocCallId();
    return new Promise<Uint8Array>((resolve, reject) => {
      const entry: PendingCall = { kind: "call", resolve, reject, cleanup: undefined };
      this.#pending.set(callId, entry);
      if (signal !== undefined) {
        const onAbort = (): void => {
          if (this.#pending.get(callId) !== entry) return;
          this.#pending.delete(callId);
          try {
            this.#transport.send(Kind.Cancel, encodeCancel({ callId }));
          } catch {
            // The channel is gone; the core cancels with it.
          }
          reject(abortReason(signal));
        };
        signal.addEventListener("abort", onAbort, { once: true });
        entry.cleanup = () => {
          signal.removeEventListener("abort", onAbort);
        };
      }
      try {
        this.#transport.send(Kind.Call, encode(callId));
      } catch (error) {
        if (this.#pending.get(callId) === entry) {
          this.#pending.delete(callId);
          entry.cleanup?.();
          reject(error);
        }
      }
    });
  }

  #openStream(target: CallTargetArg, methodId: number, args: Uint8Array): StreamCall {
    const callId = this.#closed ? 0 : this.#allocCallId();
    const stream = new StreamCall(callId, {
      sendCredit: (id, credit) => {
        this.#assertOpen();
        this.#transport.send(Kind.StreamCredit, encodeStreamCredit({ callId: id, credit }));
      },
      cancel: (id) => {
        this.#pending.delete(id);
        if (this.#closed) return;
        this.#transport.send(Kind.Cancel, encodeCancel({ callId: id }));
      },
    });
    if (this.#closed) {
      stream.fail(new KeelTransportError("closed", "the core is closed"));
      return stream;
    }
    this.#pending.set(callId, { kind: "stream", stream });
    try {
      this.#transport.send(Kind.Call, encodeTarget(target, methodId, callId, args));
    } catch (error) {
      this.#pending.delete(callId);
      stream.fail(error);
    }
    return stream;
  }

  // ----- what the transport tells us --------------------------------------------------

  readonly #handler: TransportHandler = {
    reply: (payload) => {
      this.#onReply(payload);
    },
    changeSet: (payload) => {
      this.mirror.enqueue(payload);
    },
    streamItem: (payload) => {
      this.#onStreamItem(payload);
    },
    portCall: (call) => this.#onPortCall(call),
    log: (level, target, message) => {
      this.#log(level, target, message);
    },
    closed: (error) => {
      this.#lost(error);
    },
  };

  #onReply(payload: Uint8Array): void {
    if (payload.length < 5) {
      this.#reportError("reply", new KeelTransportError("protocol", "the core sent a truncated reply"));
      return;
    }
    const callId = new DataView(payload.buffer, payload.byteOffset, payload.byteLength).getUint32(0, true);
    const status = payload[4] as number;
    const body = payload.subarray(5);
    const entry = this.#pending.get(callId);
    if (entry === undefined) return; // aborted or cancelled meanwhile, or never ours

    if (status > ReplyStatus.BadRequest) {
      this.#pending.delete(callId);
      const error = new KeelTransportError("protocol", `the core sent reply status ${status}`);
      if (entry.kind === "call") {
        entry.cleanup?.();
        entry.reject(error);
      } else {
        entry.stream.fail(error);
      }
      return;
    }

    if (entry.kind === "stream") {
      if (status === ReplyStatus.StreamOpened) {
        entry.stream.opened();
        return;
      }
      this.#pending.delete(callId);
      entry.stream.fail(
        status === ReplyStatus.Ok
          ? new KeelTransportError("protocol", "the core answered a stream call with a plain result")
          : new KeelReplyError(status as ReplyStatus, body),
      );
      return;
    }

    if (status === ReplyStatus.StreamOpened) {
      // A plain call to a stream method: free the stream the core just registered.
      this.#pending.delete(callId);
      entry.cleanup?.();
      entry.reject(new KeelError("state", "this method is a stream; call it with KeelCore.stream"));
      try {
        this.#transport.send(Kind.Cancel, encodeCancel({ callId }));
      } catch {
        // Closing anyway.
      }
      return;
    }
    this.#pending.delete(callId);
    entry.cleanup?.();
    if (status === ReplyStatus.Ok) entry.resolve(body);
    else entry.reject(new KeelReplyError(status as ReplyStatus, body));
  }

  #onStreamItem(payload: Uint8Array): void {
    if (payload.length < 5) {
      this.#reportError("stream", new KeelTransportError("protocol", "the core sent a truncated stream item"));
      return;
    }
    const callId = new DataView(payload.buffer, payload.byteOffset, payload.byteLength).getUint32(0, true);
    const flag = payload[4] as number;
    const entry = this.#pending.get(callId);
    if (entry?.kind !== "stream") return;
    const body = payload.subarray(5);
    switch (flag) {
      case StreamFlag.Item:
        entry.stream.push(body);
        return;
      case StreamFlag.End:
        this.#pending.delete(callId);
        entry.stream.end();
        return;
      case StreamFlag.Error:
        this.#pending.delete(callId);
        entry.stream.fail(new KeelReplyError(ReplyStatus.Error, body));
        return;
      default:
        this.#pending.delete(callId);
        entry.stream.fail(new KeelTransportError("protocol", `the core sent stream flag ${flag}`));
    }
  }

  #onPortCall(call: PortCallPayload): PortOutcome {
    const method = this.#ports.get(call.portId)?.methods[call.methodId];
    if (method === undefined) return UNAVAILABLE;
    let result: Uint8Array | PromiseLike<Uint8Array>;
    try {
      result = method(call.args);
    } catch (error) {
      return { kind: "sync", reply: this.#portFailure(call, error) };
    }
    if (isThenable(result)) {
      result.then(
        (body) => {
          this.#sendPortReply(encodePortReply({ portCallId: call.portCallId, status: PortStatus.Ok, body }));
        },
        (error: unknown) => {
          this.#sendPortReply(this.#portFailure(call, error));
        },
      );
      return ASYNC;
    }
    return { kind: "sync", reply: encodePortReply({ portCallId: call.portCallId, status: PortStatus.Ok, body: result }) };
  }

  /** The `PortReply` for a port method that threw: its typed error, or "unavailable" after logging anything else. */
  #portFailure(call: PortCallPayload, error: unknown): Uint8Array {
    if (error instanceof KeelPortError) {
      return encodePortReply({ portCallId: call.portCallId, status: PortStatus.Error, body: error.body });
    }
    this.#reportError(`port 0x${call.portId.toString(16)} method 0x${call.methodId.toString(16)}`, error);
    return encodePortReply({ portCallId: call.portCallId, status: PortStatus.Unavailable, body: NO_BYTES });
  }

  #sendPortReply(reply: Uint8Array): void {
    if (this.#closed) return;
    try {
      this.#transport.send(Kind.PortReply, reply);
    } catch (error) {
      this.#reportError("port reply", error);
    }
  }

  #log(level: number, target: string, message: string): void {
    try {
      (this.#adapters.log ?? consoleLog()).log(level, target, message);
    } catch {
      // A failing log sink must not break the core.
    }
  }

  #reportError(where: string, error: unknown): void {
    this.#log(4, "keel::runtime", `${where}: ${errorMessage(error)}`);
    try {
      this.#options.onError?.(error);
    } catch {
      // The reporter itself failed; nothing more can be done.
    }
  }
}
