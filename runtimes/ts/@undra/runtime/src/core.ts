import { browserAdapters } from "./adapters/browser.js";
import { standardPorts, startEventSources, timerPort } from "./adapters/ports.js";
import { PortIds } from "./adapters/ids.js";
import { WEB_CRYPTO_REQUIRED, consoleLog, hasCryptoRandom } from "./adapters/system.js";
import type { Adapters, AdapterOverrides } from "./adapters/types.js";
import {
  UndraError,
  UndraModeError,
  UndraReplyError,
  UndraSchemaMismatchError,
  UndraSessionLostError,
  UndraTransportError,
} from "./errors.js";
import { nextCallId } from "./callid.js";
import { UndraCallError, UndraUnhandledError } from "./call-error.js";
import { Mirror, type MirrorOptions, type MirrorStats } from "./mirror.js";
import type { RecreateCall, UndraStore } from "./object.js";
import {
  type RecoveryOptions,
  type ResolvedRecovery,
  type RestartResult,
  UndraCoreRestarted,
  type UndraPanicReport,
  isTrap,
  panicReport,
  resolveRecovery,
  restartedError,
} from "./recovery.js";
import { errorMessage } from "./platform.js";
import type { PortImpl } from "./port.js";
import { dispatchPortCall, portOperation, syncPortRefusal } from "./port-dispatch.js";
import { Signal } from "./signal.js";
import { StreamCall } from "./stream.js";
import { type ReconnectOptions, RemoteTransport, type WebSocketFactory } from "./transport/remote.js";
import type { PortOutcome, Transport, TransportHandler } from "./transport/transport.js";
import { WasmMainTransport, type WasmSource } from "./transport/wasm-main.js";
import type { WorkerLike } from "./transport/wasm-worker.js";
import {
  ALL_SIGNALS,
  CallTarget,
  type Handle,
  type HelloPayload,
  Kind,
  ReplyStatus,
  StreamFlag,
  type PortCallPayload,
  type StreamFailure,
  codecs,
  decodeStreamFailure,
  decodeValue,
  encodeCall,
  encodeCancel,
  encodeEvent,
  encodeObserve,
  encodeRelease,
  encodeStreamCredit,
  encodeTimerFired,
  handleGeneration,
  streamFailureReplyBody,
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

/** Live counters of a core; see {@link UndraCore.stats}. */
export interface UndraStats {
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
  /** The mirror's delivery counters: change-sets and entries received, entries applied after merging, drains, compactions, resyncs (docs/SPEC.md section 17.1). */
  readonly mirror: MirrorStats;
  /** The core's own statistics (`undra_stats_json`, parsed) when the transport can ask for them: wasm modes. `null` over a socket. */
  readonly core: Readonly<Record<string, unknown>> | null;
}

/** Why a core is `closed`: the app closed it, its schema is not the bindings', the dev server lost its session (ADR-051), or the connection failed for good. */
export type ConnectionClosedReason = "requested" | "schemaMismatch" | "sessionLost" | "failed";

/**
 * What the connection to the core is doing (`UndraCore.connection`). Only a `remote` core ever
 * leaves `connected`: it is `reconnecting` (attempt 1, 2, ... with the error that caused it) after
 * the connection drops, `connected` again once the stores are observed again, and `closed` for
 * good when the app closes it, its schema changed, the dev server lost its session, or the
 * transport gave up. A wasm core is `connected` from `load` until it is closed.
 */
export type ConnectionState =
  | { readonly kind: "connecting" }
  | { readonly kind: "connected" }
  | { readonly kind: "reconnecting"; readonly attempt: number; readonly error: Error }
  | { readonly kind: "closed"; readonly reason: ConnectionClosedReason; readonly error?: Error };

/** Options shared by `UndraCore.load` and `UndraCore.attach`. */
export interface AttachOptions {
  /** The schema hash of the generated bindings (`UndraIds.schemaHash`); a core built from another schema is refused. */
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
  /** Called once when the channel to the core is lost for good (not when you call `close()`, and not while a `remote` core is reconnecting: see `onConnectionChange`). */
  readonly onClose?: (error: Error) => void;
  /** Called with every change of {@link UndraCore.connection}, starting with `connecting`, on the thread that changed it. */
  readonly onConnectionChange?: (state: ConnectionState) => void;
  /**
   * Called with every failure that has no caller to reject (ADR-032, amendment A): a generated command (a
   * synchronous method that returns nothing and has no error type) that failed, a store change that could
   * not be applied, a malformed change-set, a port that failed. The failure is also logged at error level,
   * whether or not a handler is set. The handler runs synchronously where the failure was found (inside a
   * core callback for a malformed change-set or a failed port): keep it short and do not call into Undra
   * from it. A failure reported while the handler runs is only logged, and so is the failure of a call the
   * handler started (a command fails after the handler returned), so a handler that calls a failing command
   * is not called again for it. An exception it throws is logged and dropped.
   */
  readonly onError?: (error: UndraUnhandledError) => void;
  /** Make this core `UndraCore.shared` when none is set yet. Default `true`. */
  readonly shared?: boolean;
  /**
   * Restart a wasm core that trapped from its last snapshot (ADR-049), off by default. `true` takes the defaults of
   * {@link RecoveryOptions}: a snapshot at most once a second while stores change (4 MiB at most, kept outside wasm
   * memory, in the worker in `wasm-worker` mode), and at most 3 restarts a minute. On a trap: `onPanic` hears the
   * panic; every call and stream in flight fails with `UndraTransportError("restarted")` (it may or may not have run;
   * it is not retried); the same compiled module is instantiated again and the snapshot restored (stores keep their
   * handles); every observed store is observed again; query handles are re-created (their wrappers move to the new
   * handles); then `onCoreRestarted` and `onError` hear an {@link UndraCoreRestarted}. Store writes after the last
   * snapshot, objects that are not stores (query handles excepted) and the core's running tasks and timers are lost.
   * One trap more than `maxRestarts` within `perMs` and the core stays dead: `onClose` reports the trap, as without
   * recovery. Wasm modes only (a native core contains its panics).
   */
  readonly recovery?: boolean | RecoveryOptions;
  /** Called after a wasm core trapped and was restarted (`recovery`), with what happened; `onError` receives the same value. */
  readonly onCoreRestarted?: (event: UndraCoreRestarted) => void;
  /**
   * Called once per trap of a wasm core with its panic report (ADR-046 decision 4.4: the core's FATAL `undra::panic`
   * record and the trap's stack), before any restart, with or without `recovery`: the place to forward a core panic to
   * a crash reporter. A handler that throws is logged.
   */
  readonly onPanic?: (report: UndraPanicReport) => void;
  /**
   * How the mirror delivers change-sets (docs/SPEC.md section 11): `schedule` replaces the frame
   * scheduler (`scheduleFrame`) that drains what the core produced on its own, and
   * `maxPendingEntries` (default 65,536) / `maxPendingBytes` (default 16 MiB) bound the backlog.
   */
  readonly mirror?: Pick<MirrorOptions, "schedule" | "maxPendingEntries" | "maxPendingBytes">;
}

/** Options of `UndraCore.load`. */
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
  /** Reconnect a `"remote"` core by itself when its connection drops: `false` turns it off, an object tunes the backoff. Default on. See {@link ReconnectOptions}. */
  readonly reconnect?: boolean | ReconnectOptions;
  /**
   * For `"wasm-worker"`: the Worker, a function creating it, or {@link WorkerModeOptions} (the Worker and the
   * module of ports that run inside it). Default a module worker on `@undra/runtime/worker`.
   */
  readonly worker?: WorkerLike | (() => WorkerLike) | WorkerModeOptions;
}

/** The `worker` option of `UndraCore.load` in `"wasm-worker"` mode, in full (ADR-049). */
export interface WorkerModeOptions {
  /** The Worker, or a function creating it; default a module worker on `@undra/runtime/worker`. */
  readonly create?: WorkerLike | (() => WorkerLike);
  /**
   * The URL of a module the worker imports before `undra_init`. Its default export maps port ids to
   * implementations (generated `<Name>PortImpl` adapters, `clockPort(...)`, as `registerPort` takes them) that
   * run inside the worker: that is where a synchronous port of the app must be registered (the core cannot wait
   * for the main thread, so registering one on the main thread is an error), and how Clock or Rng are overridden.
   * An `adapters` export (`{ clock?, rng?, timer? }`) backs the core's clock, random source and timers there. The
   * URL must be loadable by the worker as an ES module (a bundler's worker entry, or a plain `.js` file).
   */
  readonly ports?: URL | string;
}

/** Whether a `worker` option is a Worker (or a stand-in with `postMessage`) rather than {@link WorkerModeOptions}. */
function isWorkerLike(option: object): option is WorkerLike {
  return typeof (option as { postMessage?: unknown }).postMessage === "function";
}

/** Statistics counters exposed by `undra_stats_json` that this runtime reads. */
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

const UNLOADED_MESSAGE = "no UndraCore is loaded: call UndraCore.load(...) at app startup, before creating any Undra object, or pass a core explicitly";
const DEFAULT_OBSERVE_TIMEOUT_MS = 10_000;

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

/** Whether `error` means that a newer trap overtook a restart in progress: the trap itself, or what it failed the waiting work with. */
function overtaken(error: unknown): boolean {
  return isTrap(error) || (error instanceof UndraTransportError && error.reason === "restarted");
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
 * The host side of an Undra core (docs/SPEC.md sections 11 and 17.1): owns the
 * transport, allocates call ids and routes replies to the promises and streams
 * that wait for them, holds the {@link Mirror} that applies change-sets, runs
 * the host's port implementations, and enforces the schema-hash check.
 *
 * ```ts
 * const core = await UndraCore.load({
 *   mode: "wasm-main",
 *   wasm: new URL("./todo_core.wasm", import.meta.url),
 *   expectedSchemaHash: UndraIds.schemaHash,
 * });
 * const todos = await Todos.create();   // uses UndraCore.shared
 * ```
 *
 * Generated code calls `call`, `callSync`, `stream`, `construct`, `observe`,
 * `release`, `event`, `registerPort` and reads `mirror` and `shared`; the rest
 * is for applications and tests.
 */
export class UndraCore {
  static #shared: UndraCore | null = null;

  /**
   * The core that the generated constructors and functions default to: the first one loaded.
   *
   * Using it before a successful `load`, or after the shared core was closed, is a programming error but not
   * a crash (ADR-032, amendment A): it returns a permanently closed placeholder whose calls reject with
   * `UndraCallError.Unavailable`, whose commands only log, and whose first use logs what to do.
   * {@link UndraCore.current} still returns `null` in that state, so check it, not this, to learn whether a
   * core is loaded.
   */
  static get shared(): UndraCore {
    return UndraCore.#shared ?? UndraCore.#placeholder();
  }

  /** The shared core, or `null` if none is loaded. While it is `null`, {@link UndraCore.shared} is the closed placeholder. */
  static get current(): UndraCore | null {
    return UndraCore.#shared;
  }

  static #unloaded: UndraCore | null = null;

  /** The placeholder `shared` returns while no core is loaded: a core that was closed from the start. */
  static #placeholder(): UndraCore {
    if (UndraCore.#unloaded === null) {
      const gone = (): never => {
        throw new UndraTransportError("closed", UNLOADED_MESSAGE);
      };
      const core = new UndraCore(
        {
          mode: "wasm-main",
          synchronous: true,
          start: () => Promise.reject(new UndraTransportError("closed", UNLOADED_MESSAGE)),
          send: gone,
          callSync: gone,
          close: () => {},
        },
        { expectedSchemaHash: 0n, shared: false },
        {},
      );
      core.#closed = true;
      core.#closedMessage = UNLOADED_MESSAGE;
      UndraCore.#unloaded = core;
      consoleLog().log(
        4,
        "undra::runtime",
        "UndraCore.shared was used while no core is loaded (before UndraCore.load(...) succeeds, or after the shared core was closed); calls on it reject with UndraCallError.Unavailable. Load a core at app startup, before creating any Undra object.",
      );
    }
    return UndraCore.#unloaded;
  }

  /**
   * Loads a core: creates the transport of `options.mode`, performs the
   * handshake and resolves with the running core. Rejects with
   * `UndraSchemaMismatchError` when the core was built from another schema than
   * `expectedSchemaHash`, and with `UndraTransportError` when it cannot be
   * reached or started. The first core loaded becomes {@link UndraCore.shared}.
   *
   * The wasm modes need WebCrypto (`crypto.getRandomValues`): without it `load`
   * rejects with `UndraTransportError("unsupported", "WebCrypto is required ...")`
   * before anything is instantiated (ADR-049), unless `adapters.rng` supplies the
   * random source (`wasm-main` only).
   */
  static async load(options: LoadOptions): Promise<UndraCore> {
    const adapters = mergeAdapters(browserAdapters(), options.adapters);
    const recovery = resolveRecovery(options.recovery);
    let transport: Transport;
    switch (options.mode) {
      case "wasm-main": {
        if (options.wasm === undefined) throw new UndraError("options", "mode 'wasm-main' needs the `wasm` option");
        // The core's only random source is the `random` import: refuse before instantiating rather than let its
        // `Rng` fail at the first idempotency key (ADR-049). An app that supplies its own `rng` adapter has one.
        if (options.adapters?.rng == null && !hasCryptoRandom()) throw new UndraTransportError("unsupported", WEB_CRYPTO_REQUIRED);
        transport = new WasmMainTransport({
          wasm: options.wasm,
          expectedSchemaHash: options.expectedSchemaHash,
          ...(options.platform !== undefined && { platform: options.platform }),
          ...(options.devtools !== undefined && { devtools: options.devtools }),
          ...(options.logLevel !== undefined && { logLevel: options.logLevel }),
          ...(adapters.clock && { clock: adapters.clock }),
          ...(adapters.rng && { rng: adapters.rng }),
          ...(adapters.timer && { timer: adapters.timer }),
          ...(recovery !== null && { recovery: { snapshotEveryMs: recovery.snapshotEveryMs, maxSnapshotBytes: recovery.maxSnapshotBytes } }),
          onError: (error) => {
            adapters.log?.log(4, "undra::runtime", `import failed: ${errorMessage(error)}`);
          },
        });
        break;
      }
      case "wasm-worker": {
        if (options.wasm === undefined) throw new UndraError("options", "mode 'wasm-worker' needs the `wasm` option");
        // Checked here, before the worker is spawned; the worker reads its own `crypto` (ADR-049).
        if (!hasCryptoRandom()) throw new UndraTransportError("unsupported", WEB_CRYPTO_REQUIRED);
        const { WasmWorkerTransport } = await import("./transport/wasm-worker.js");
        const worker = options.worker;
        const modeOptions: WorkerModeOptions = worker === undefined ? {} : typeof worker === "function" || isWorkerLike(worker) ? { create: worker } : worker;
        transport = new WasmWorkerTransport({
          wasm: options.wasm,
          expectedSchemaHash: options.expectedSchemaHash,
          ...(modeOptions.create !== undefined && { worker: modeOptions.create }),
          ...(modeOptions.ports !== undefined && { ports: modeOptions.ports }),
          ...(recovery !== null && { recovery: { snapshotEveryMs: recovery.snapshotEveryMs, maxSnapshotBytes: recovery.maxSnapshotBytes } }),
          ...(options.platform !== undefined && { platform: options.platform }),
          ...(options.devtools !== undefined && { devtools: options.devtools }),
          ...(options.logLevel !== undefined && { logLevel: options.logLevel }),
          ...(options.handshakeTimeoutMs !== undefined && { startTimeoutMs: options.handshakeTimeoutMs }),
        });
        break;
      }
      case "remote": {
        if (options.url === undefined) throw new UndraError("options", "mode 'remote' needs the `url` option");
        transport = new RemoteTransport({
          url: options.url,
          expectedSchemaHash: options.expectedSchemaHash,
          ...(options.platform !== undefined && { platform: options.platform }),
          ...(options.devtools !== undefined && { devtools: options.devtools }),
          ...(options.webSocket !== undefined && { webSocket: options.webSocket }),
          ...(options.handshakeTimeoutMs !== undefined && { handshakeTimeoutMs: options.handshakeTimeoutMs }),
          ...(options.reconnect !== undefined && { reconnect: options.reconnect }),
        });
        break;
      }
      default:
        throw new UndraError("options", `unknown mode '${String((options as { mode: unknown }).mode)}'`);
    }
    return UndraCore.#attach(transport, options, adapters);
  }

  /**
   * Runs a core over a transport you provide (an embedder's IPC channel, a
   * test double) instead of one of the built-in modes. Everything else is as
   * for {@link UndraCore.load}, including the schema check on the transport's
   * `Hello`.
   */
  static attach(transport: Transport, options: AttachOptions): Promise<UndraCore> {
    return UndraCore.#attach(transport, options, mergeAdapters(browserAdapters(), options.adapters));
  }

  static async #attach(transport: Transport, options: AttachOptions, adapters: Partial<Adapters>): Promise<UndraCore> {
    const core = new UndraCore(transport, options, adapters);
    try {
      await core.#start();
    } catch (error) {
      core.#dispose(null);
      throw error;
    }
    if (options.shared !== false && UndraCore.#shared === null) UndraCore.#shared = core;
    return core;
  }

  /** The mirror that applies change-sets to stores; stores register themselves with it. */
  readonly mirror: Mirror;
  /** What the core said in its `Hello` (for wasm modes, synthesised from the module). Set once `load` resolves. */
  hello: HelloPayload = { undraVersion: "", schemaHash: 0n, platform: "", mode: "" };

  readonly #transport: Transport;
  readonly #options: AttachOptions;
  readonly #adapters: Partial<Adapters>;
  readonly #observeTimeoutMs: number;
  readonly #ports = new Map<number, PortImpl>();
  readonly #pending = new Map<number, PendingCall | PendingStream>();
  readonly #handles = new Set<Handle>();
  /** The signals the app observes, per handle: what a reconnect observes again. */
  readonly #observed = new Map<Handle, Set<number>>();
  /** Handles released while the connection was down: released in the core once it is back. */
  readonly #releasedWhileDown = new Set<Handle>();
  readonly #connection = new Signal<ConnectionState>({ kind: "connecting" });
  #nextCallId = 0;
  #closed = false;
  /** What a call on this closed core says; the default is "the core is closed". */
  #closedMessage = "the core is closed";
  #reporting = false;
  /** The failures of calls the `onError` handler started (see `report`): reported, they are only logged. */
  readonly #handlerFailures = new WeakSet<object>();
  #stopEvents: (() => void) | null = null;
  /** Whether `start` succeeded (the transport is running). */
  #started = false;
  /** Bumped by every restart (ADR-049): a port reply that settles later belongs to the epoch of its call. */
  #epoch = 0;
  /** `LoadOptions.recovery`, resolved; `null` when off. */
  readonly #recovery: ResolvedRecovery | null;
  /** While a trapped core is being instantiated again: calls fail with "restarted" (ADR-049). */
  #restarting = false;
  /** When the restarts within the budget's window happened (`Date.now`). */
  #restartTimes: number[] = [];
  /** Counts recoveries: a re-attach that a newer trap overtook stops. */
  #recoveries = 0;
  /** The message of the last FATAL `undra::panic` record: what a trap's panic report says (ADR-046). */
  #lastPanicRecord: string | null = null;
  /** The stores the runtime re-creates after a restart instead of restoring them (query handles), by handle. */
  readonly #recreatable = new Map<Handle, { readonly ref: WeakRef<UndraStore>; readonly call: RecreateCall }>();

  private constructor(transport: Transport, options: AttachOptions, adapters: Partial<Adapters>) {
    this.#transport = transport;
    this.#options = options;
    this.#adapters = adapters;
    this.#recovery = resolveRecovery(options.recovery);
    this.#notifyConnection(this.#connection.peek());
    this.#observeTimeoutMs = options.observeTimeoutMs ?? DEFAULT_OBSERVE_TIMEOUT_MS;
    this.mirror = new Mirror({
      ...options.mirror,
      onError: (error) => {
        this.#reportError("mirror", error);
      },
      resync: (handle, signalId) => {
        this.#resync(handle, signalId);
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

  /**
   * What the connection to the core is doing, as a signal: `connecting` until `load` resolves,
   * `connected`, `reconnecting` with the attempt number after a `remote` connection drops, and
   * `closed` with its reason. `useSignal(core.connection)` renders it in React.
   *
   * While it is `reconnecting`, calls and `observe` fail at once with an `UndraTransportError`
   * (`"closed"`, which a generated call rejects with as `UndraCallError.Unavailable`); what was in
   * flight when the connection dropped failed with the same. When it is
   * `connected` again every store the app observes has been observed again, so the mirrors converge
   * on the core's current values by themselves.
   */
  get connection(): Signal<ConnectionState> {
    return this.#connection;
  }

  // ----- calls -----------------------------------------------------------------------

  /**
   * Calls a synchronous method and returns its result directly. Available in
   * `wasm-main` only; every other mode throws {@link UndraModeError}. Rejects
   * asynchronous methods (the core answers status 5). Throws
   * {@link UndraReplyError} when the call does not succeed. The change-sets
   * the call produced are applied to the stores before it returns, except
   * when it is made from inside a drain (a signal subscriber): the running
   * drain applies them in its next round, after the subscriber returns.
   */
  callSync(target: CallTargetArg, methodId: number, args: Uint8Array): Uint8Array {
    const transport = this.#transport;
    if (transport.callSync === undefined) throw new UndraModeError("callSync", transport.mode);
    this.#assertOpen();
    let reply: Uint8Array;
    try {
      reply = transport.callSync(encodeTarget(target, methodId, this.#allocCallId(), args));
    } catch (error) {
      throw this.#failureOf(error);
    }
    // Read-your-writes (docs/SPEC.md section 11): the call's change-sets are queued by now.
    this.mirror.flush();
    if (reply.length < 5) throw new UndraTransportError("protocol", "the core returned a truncated reply");
    const status = reply[4] as number;
    const body = reply.subarray(5);
    if (status === ReplyStatus.Ok) return body;
    throw new UndraReplyError(status as ReplyStatus, body);
  }

  /**
   * Calls a method and resolves with the reply body. Rejects with
   * {@link UndraReplyError} (status 1 typed error, 2 panic, 3 cancelled, 5 bad
   * request) or {@link UndraTransportError}. When `signal` aborts, the call is
   * cancelled in the core (`Cancel`) and the promise rejects at once with the
   * signal's reason; an already aborted signal never sends anything. The
   * change-sets that arrived before the reply are applied before the promise
   * settles, so the code after `await` sees them.
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
   * stream rejects with {@link UndraReplyError} exactly like a failed call:
   * status 1 with the encoded `E` when the stream ends with its own typed
   * error, status 2, 3 or 5 with the section 3.4 body when the core reports
   * that it panicked, cancelled the stream or refused it (ADR-036).
   */
  stream(target: CallTargetArg, methodId: number, args: Uint8Array): AsyncIterable<Uint8Array> {
    return { [Symbol.asyncIterator]: () => this.#openStream(target, methodId, args) };
  }

  /**
   * Runs a constructor (`typeId` names the object type, `methodId` the
   * constructor) and resolves with the new object's handle. Rejects like `call`,
   * and with {@link UndraTransportError} (`"protocol"`) when the core answers
   * with the null handle.
   */
  async construct(typeId: number, methodId: number, args: Uint8Array): Promise<Handle> {
    const body = await this.#request((callId) =>
      encodeCall({ target: CallTarget.Constructor, typeId, methodId, callId, args }),
    );
    const handle = decodeValue(codecs.u64, body);
    if (handle === 0n) throw new UndraTransportError("protocol", "the core returned the null handle for a constructor");
    this.#handles.add(handle);
    return handle;
  }

  /**
   * Starts (`on`) or stops observing a signal of a store (`ALL_SIGNALS` for
   * every one). Starting resolves once the initial values have reached the
   * mirror and been applied to the store's signals: at once for an in-process
   * core (the core delivers them before `observe` returns), when the first
   * change-set entry for the observation has been applied for a worker or
   * remote core. The latter rejects with `UndraError("observe")` after
   * `observeTimeoutMs` if none arrives, which means the handle is not a live
   * store.
   */
  observe(handle: Handle, signalId: number, on: boolean): Promise<void> {
    try {
      this.#assertOpen();
      this.#transport.send(Kind.Observe, encodeObserve({ handle, signalId, on }));
    } catch (error) {
      return Promise.reject(this.#failureOf(error));
    }
    this.#noteObserved(handle, signalId, on);
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
    this.#observed.delete(handle);
    this.#recreatable.delete(handle);
    if (this.#closed) return;
    if (this.#connection.peek().kind === "reconnecting" || this.#restarting) {
      // The core keeps the object for us (ADR-051); it is released when the connection is back.
      this.#releasedWhileDown.add(handle);
      return;
    }
    try {
      this.#transport.send(Kind.Release, encodeRelease({ handle }));
    } catch (error) {
      this.#reportError("release", error);
    }
  }

  /** Remembers what the app observes, so that a reconnect can observe it again. */
  #noteObserved(handle: Handle, signalId: number, on: boolean): void {
    if (on) {
      let signals = this.#observed.get(handle);
      if (signals === undefined) this.#observed.set(handle, (signals = new Set()));
      signals.add(signalId);
    } else if (signalId === ALL_SIGNALS) {
      this.#observed.delete(handle);
    } else {
      const signals = this.#observed.get(handle);
      signals?.delete(signalId);
      if (signals?.size === 0) this.#observed.delete(handle);
    }
  }

  /** Sends a host-to-core event of an event port (`Connectivity.changed`, `Lifecycle.changed`, ...). Throws {@link UndraTransportError} when the core is closed. */
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
   *
   * In `wasm-worker` mode a synchronous port (`impl.sync`) cannot be served from
   * this thread, because the core cannot wait for it: registering one throws
   * `UndraError("options")` naming the port and the fix (register it in the
   * worker, in the module of `LoadOptions.worker.ports`, ADR-049). An
   * asynchronous port registered after load is announced to the worker.
   */
  registerPort(portId: number, impl: PortImpl): void {
    if (impl.sync && this.#transport.answersSyncPorts === false) throw new UndraError("options", syncPortRefusal(portId));
    this.#ports.set(portId, impl);
    if (!this.#closed && this.#started) this.#transport.portsChanged?.(this.#asyncPortIds());
  }

  /** The ids of the registered ports whose methods answer asynchronously: what the worker of `wasm-worker` forwards to this thread. */
  #asyncPortIds(): number[] {
    const ids: number[] = [];
    for (const [portId, impl] of this.#ports) if (!impl.sync) ids.push(portId);
    return ids;
  }

  /**
   * Reports a failure that no caller can see (ADR-032, amendment A): logs it at error level and passes it to
   * `onError`. Generated commands and store `_apply` call it; it never throws and never rejects.
   *
   * `error` is mapped the way a rejecting call's error is (`UndraCallError.mapped`), so the handler always
   * receives an `UndraCallError` inside the {@link UndraUnhandledError} (a failure that is not Undra's is
   * `Malformed`, with the original as the `cause`). A failure that is a `remote` core's connection being down
   * (`Unavailable` while {@link UndraCore.connection} is `reconnecting`, or `closed` for a reason other than
   * `requested`) is only logged, at warning level: the connection state and `onConnectionChange` already report
   * it, once, and a command tapped meanwhile is not a second failure to hand to a crash reporter. A report made
   * while the handler runs is only logged,
   * and so is the failure of a call the handler started: a command the handler calls fails after the
   * handler returned (every method is asynchronous), and reporting it again would call the handler again,
   * for ever.
   *
   * @param error What the call threw.
   * @param operation What failed, as TypeScript spells it, for example `"Todos.toggle"`.
   */
  report(error: unknown, operation: string): void {
    const unhandled = new UndraUnhandledError(operation, UndraCallError.asCallError(error), error);
    if (this.#isConnectionDown(unhandled.error)) {
      this.#log(3, "undra::runtime", `${unhandled.message} (the connection to the core is down: see UndraCore.connection)`);
      return;
    }
    this.#log(4, "undra::runtime", unhandled.message);
    const handler = this.#options.onError;
    if (handler === undefined || this.#reporting || this.#startedByHandler(error)) return;
    this.#reporting = true;
    try {
      handler(unhandled);
    } catch (thrown) {
      this.#log(4, "undra::runtime", `the onError handler threw while handling "${unhandled.message}": ${errorMessage(thrown)}`);
    } finally {
      this.#reporting = false;
    }
  }

  /**
   * Whether `error` is the connection of a `remote` core being down, which {@link UndraCore.connection} already reports:
   * the core is `reconnecting`, or `closed` for a reason other than the app's own `close()`. A wasm core that trapped is
   * not a connection, and a core the app closed is a programming error: both are still reported.
   */
  #isConnectionDown(error: UndraCallError): boolean {
    if (error.kind !== "unavailable" || this.#transport.mode !== "remote") return false;
    const state = this.#connection.peek();
    return state.kind === "reconnecting" || (state.kind === "closed" && state.reason !== "requested");
  }

  /** Live counters of this core; see {@link UndraStats}. */
  async stats(): Promise<UndraStats> {
    let core: CoreStatsJson | null = null;
    const json = this.#closed || this.#restarting ? null : await this.#transport.stats?.().catch(() => null);
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
      mirror: this.mirror.stats(),
      core,
    };
  }

  /**
   * The persisted state of every store (docs/SPEC.md section 5.9), as opaque bytes that `restore`
   * accepts, also into a core loaded later from the same module. Objects that are not stores are
   * not part of it. In `wasm-worker` mode the snapshot is taken by the worker behind the messages
   * sent before it. Rejects with {@link UndraTransportError} when the core is closed, and with
   * {@link UndraModeError} over a transport that cannot snapshot (`remote`).
   */
  async snapshot(): Promise<Uint8Array> {
    this.#assertOpen();
    const transport = this.#transport;
    if (transport.snapshot === undefined) throw new UndraModeError("snapshot", transport.mode);
    return transport.snapshot();
  }

  /**
   * Rebuilds the stores from `bytes` (a `snapshot`); the handles the app holds stay valid. Resolves
   * after the restored values reached the stores (the core re-delivers the observed signals, and
   * the mirror is flushed, so code after `await core.restore(..)` reads the restored values). A
   * call or stream in flight on a store the restore replaced ends as cancelled by the core
   * (reply status 3; a stream ends with a flag-3 failure of status 3); an object that is not a store becomes a
   * stale handle. Rejects with `UndraRestoreError` when the core refuses the bytes, in which case
   * it is unchanged and still usable, with {@link UndraTransportError} when the core is closed, and
   * with {@link UndraModeError} over a transport that cannot restore (`remote`).
   */
  async restore(bytes: Uint8Array): Promise<void> {
    this.#assertOpen();
    const transport = this.#transport;
    if (transport.restore === undefined) throw new UndraModeError("restore", transport.mode);
    await transport.restore(bytes);
    // Read-your-writes (docs/SPEC.md section 11): what the restore delivered is applied before the caller resumes.
    this.mirror.flush();
  }

  /**
   * Closes the core: in-flight calls and streams reject with
   * {@link UndraTransportError}, event sources stop, the transport is released.
   * Idempotent. Later calls reject or throw.
   */
  close(): void {
    this.#dispose(new UndraTransportError("closed", "the core was closed"));
  }

  // ----- internals -------------------------------------------------------------------

  async #start(): Promise<void> {
    if (this.#transport.answersSyncPorts === false) this.#checkWorkerPorts();
    const hello = await this.#transport.start(this.#handler);
    this.#started = true;
    if (hello.schemaHash !== this.#options.expectedSchemaHash) {
      throw new UndraSchemaMismatchError(this.#options.expectedSchemaHash, hello.schemaHash);
    }
    this.hello = hello;
    this.#setConnection({ kind: "connected" });
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

  /**
   * A transport whose core cannot wait for this thread (`wasm-worker`): a synchronous port registered here is a
   * load-time error (ADR-049), and a Clock, Rng or Timer adapter given explicitly is said not to reach the core.
   */
  #checkWorkerPorts(): void {
    for (const [portId, impl] of this.#ports) {
      if (impl.sync) throw new UndraError("options", syncPortRefusal(portId));
    }
    const given = (["clock", "rng", "timer"] as const).filter((name) => this.#options.adapters?.[name] != null);
    if (given.length > 0) {
      this.#log(
        3,
        "undra::worker",
        `adapters.${given.join(", adapters.")} do not reach the core in wasm-worker mode: the worker answers Clock and Rng itself and owns its timers; to override them, export them from the module of LoadOptions.worker.ports`,
      );
    }
  }

  #assertOpen(): void {
    if (this.#closed) throw new UndraTransportError("closed", this.#closedMessage);
    if (this.#restarting) {
      throw new UndraTransportError("restarted", "the wasm core trapped and is restarting from its last snapshot; try again in a moment");
    }
  }

  /** A transport failure as a caller sees it: a trap the core recovers from is "restarted" (ADR-049). */
  #failureOf(error: unknown): unknown {
    return isTrap(error) && (this.#restarting || this.#mayRestart()) ? restartedError(error) : error;
  }

  /** Stops everything. `reason` is what pending work fails with; `null` when there is none (a failed start). */
  #dispose(reason: Error | null, why: ConnectionClosedReason = reason === null ? "failed" : "requested"): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#restarting = false;
    if (UndraCore.#shared === this) UndraCore.#shared = null;
    this.#stopEvents?.();
    this.#stopEvents = null;
    this.#failInFlight(reason ?? new UndraTransportError("closed", "the core is closed"));
    this.#setConnection(reason === null || why === "requested" ? { kind: "closed", reason: why } : { kind: "closed", reason: why, error: reason });
    this.#transport.close();
  }

  /** Fails every call, stream and `observe` that waits for the core with `failure`; returns how many calls and streams there were. */
  #failInFlight(failure: Error): number {
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
    return pending.length;
  }

  /** The channel to the core was lost: a trap the core recovers from (ADR-049), or for good. */
  #lost(error: Error): void {
    // A trap while a restart is under way is the restart's to handle (it failed, and says so).
    if (this.#closed || this.#restarting) return;
    if (isTrap(error)) {
      const report = this.#panicReport(error);
      if (this.#mayRestart()) {
        void this.#recover(error, report);
        return;
      }
    }
    this.#lostForGood(error);
  }

  #lostForGood(error: Error): void {
    if (this.#closed) return;
    const why: ConnectionClosedReason =
      error instanceof UndraSchemaMismatchError ? "schemaMismatch" : error instanceof UndraSessionLostError ? "sessionLost" : "failed";
    this.#dispose(error, why);
    try {
      this.#options.onClose?.(error);
    } catch (thrown) {
      this.#reportError("onClose", thrown);
    }
  }

  // ----- recovery (ADR-049 decision 3) -------------------------------------------------

  /**
   * Registers a store the runtime re-creates after a restart instead of restoring it (a query handle): its constructor
   * call is recorded, and after a restart it runs again and the store moves to the new handle.
   *
   * @internal Called by `UndraStore` for the `recreate` option that generated query handles pass.
   */
  _recreatable(store: UndraStore, call: RecreateCall): void {
    this.#recreatable.set(store.handle, { ref: new WeakRef(store), call });
  }

  /** The panic report of `trap` (ADR-046 decision 4.4), handed to `onPanic`. */
  #panicReport(trap: Error): UndraPanicReport {
    const report = panicReport(this.#lastPanicRecord, trap, this.hello.schemaHash, this.#transport.mode);
    this.#lastPanicRecord = null;
    const handler = this.#options.onPanic;
    if (handler !== undefined) {
      try {
        handler(report);
      } catch (thrown) {
        this.#log(4, "undra::runtime", `the onPanic handler threw: ${errorMessage(thrown)}`);
      }
    }
    return report;
  }

  /** Whether a trap now would be recovered from: recovery is on, the transport can restart, and the budget allows one more. */
  #mayRestart(): boolean {
    const recovery = this.#recovery;
    if (recovery === null || this.#transport.restart === undefined || this.#closed) return false;
    const since = Date.now() - recovery.perMs;
    this.#restartTimes = this.#restartTimes.filter((at) => at > since);
    return this.#restartTimes.length < recovery.maxRestarts;
  }

  /** The highest handle generation the host has seen: the floor a restore must not go below, so no live handle is issued again (ADR-022). */
  #generationFloor(): number {
    let floor = 0;
    const note = (handle: Handle): void => {
      floor = Math.max(floor, handleGeneration(handle));
    };
    for (const handle of this.#handles) note(handle);
    for (const handle of this.#observed.keys()) note(handle);
    for (const handle of this.#recreatable.keys()) note(handle);
    for (const handle of this.#releasedWhileDown) note(handle);
    return floor;
  }

  /**
   * The restart sequence of ADR-049 decision 3.4: the panic report went to `onPanic` already; every call and stream in
   * flight fails with "restarted"; the transport instantiates the same module again and restores the last snapshot;
   * the stores are observed again and the query handles re-created; then `onCoreRestarted` and `onError`. A trap
   * during it counts against the budget like any other.
   */
  async #recover(firstTrap: UndraTransportError, firstReport: UndraPanicReport): Promise<void> {
    const run = ++this.#recoveries;
    let trap = firstTrap;
    let report = firstReport;
    this.#restarting = true;
    this.#epoch++;
    const rejectedCalls = this.#failInFlight(restartedError(trap));
    const restart = this.#transport.restart as (floor: number) => Promise<RestartResult>;
    let result: RestartResult;
    for (;;) {
      this.#restartTimes.push(Date.now());
      try {
        result = await restart.call(this.#transport, this.#generationFloor());
        break;
      } catch (error) {
        if (this.#closed || run !== this.#recoveries) return;
        if (isTrap(error)) {
          trap = error;
          report = this.#panicReport(error);
          if (this.#mayRestart()) continue;
        }
        this.#restarting = false;
        this.#log(4, "undra::recovery", `the wasm core could not be restarted: ${errorMessage(error)}`);
        this.#lostForGood(isTrap(error) ? error : trap);
        return;
      }
    }
    if (this.#closed || run !== this.#recoveries) return;
    this.hello = result.hello;
    this.#restarting = false;
    let staleObjects: number | null;
    try {
      staleObjects = await this.#reattach(result, run);
    } catch (error) {
      // A trap while observing again: the transport reported it, and that report starts the next round.
      if (!overtaken(error) && !this.#closed) this.#reportError("core restart", error);
      return;
    }
    if (staleObjects === null || this.#closed || run !== this.#recoveries) return;
    const event = new UndraCoreRestarted({ report, restoredFromAgeMs: result.restoredFromAgeMs, rejectedCalls, staleObjects }, trap);
    this.#log(
      3,
      "undra::recovery",
      `the wasm core trapped (${report.message}) and was restarted from ${result.restoredFromAgeMs === null ? "no snapshot" : `a snapshot ${result.restoredFromAgeMs} ms old`}: ${rejectedCalls} call(s) in flight failed, ${staleObjects} object(s) went stale`,
    );
    this.#notifyRestarted(event);
  }

  /**
   * After a restart: what was released meanwhile is released, the stores the snapshot brought back are observed again
   * (their values reach the mirror), and the query handles are re-created and their wrappers moved to the new handles.
   * Returns how many objects went stale, or `null` when a newer trap overtook this round.
   */
  async #reattach(result: RestartResult, run: number): Promise<number | null> {
    const restored = result.storeHandles === null ? null : new Set(result.storeHandles);
    const recreate = [...this.#recreatable.entries()];
    const recreated = new Set(recreate.map(([handle]) => handle));
    let stale = 0;
    for (const handle of new Set([...this.#handles, ...this.#observed.keys()])) {
      if (recreated.has(handle) || restored === null || restored.has(handle)) continue;
      stale++;
      this.#handles.delete(handle);
      this.#observed.delete(handle);
    }
    for (const handle of this.#releasedWhileDown) {
      try {
        this.#transport.send(Kind.Release, encodeRelease({ handle }));
      } catch {
        // Gone with the instance it belonged to.
      }
    }
    this.#releasedWhileDown.clear();
    const observing: Array<Promise<void>> = [];
    for (const [handle, signals] of [...this.#observed]) {
      if (recreated.has(handle)) continue;
      for (const signalId of signals) observing.push(this.observe(handle, signalId, true));
    }
    const settled = await Promise.allSettled(observing);
    if (run !== this.#recoveries || this.#closed) return null;
    const trapped = settled.find((s): s is PromiseRejectedResult => s.status === "rejected" && overtaken(s.reason));
    if (trapped !== undefined) throw trapped.reason;
    for (const [old, entry] of recreate) {
      const store = entry.ref.deref();
      this.#recreatable.delete(old);
      if (store === undefined || store.closed) continue;
      let handle: Handle;
      try {
        handle = await this.construct(entry.call.typeId, entry.call.methodId, entry.call.args);
      } catch (error) {
        if (run !== this.#recoveries || this.#closed) return null;
        if (overtaken(error)) throw error;
        stale++;
        this.#reportError(`re-creating a query handle after a restart (type 0x${entry.call.typeId.toString(16)})`, error);
        continue;
      }
      if (run !== this.#recoveries || this.#closed || store.closed) {
        this.release(handle);
        if (run !== this.#recoveries || this.#closed) return null;
        continue;
      }
      const signals = this.#observed.get(old) ?? new Set<number>();
      this.#observed.delete(old);
      this.#handles.delete(old);
      this.mirror.move(old, handle);
      store._rebind(handle);
      this.#recreatable.set(handle, entry);
      const outcomes = await Promise.allSettled([...signals].map((signalId) => this.observe(handle, signalId, true)));
      if (run !== this.#recoveries || this.#closed) return null;
      const failed = outcomes.find((o): o is PromiseRejectedResult => o.status === "rejected");
      if (failed !== undefined) {
        if (overtaken(failed.reason)) throw failed.reason;
        this.#reportError("observing a re-created query handle after a restart", failed.reason);
      }
    }
    return stale;
  }

  /** Tells the app about a restart: `onCoreRestarted`, then `onError` with the same value (each guarded). */
  #notifyRestarted(event: UndraCoreRestarted): void {
    try {
      this.#options.onCoreRestarted?.(event);
    } catch (thrown) {
      this.#log(4, "undra::runtime", `the onCoreRestarted handler threw: ${errorMessage(thrown)}`);
    }
    const handler = this.#options.onError;
    if (handler === undefined || this.#reporting) return;
    this.#reporting = true;
    try {
      handler(event);
    } catch (thrown) {
      this.#log(4, "undra::runtime", `the onError handler threw while handling "${event.message}": ${errorMessage(thrown)}`);
    } finally {
      this.#reporting = false;
    }
  }

  /** The connection dropped and the transport reconnects: what was in flight is lost, the core stays open. */
  #reconnecting(attempt: number, error: Error): void {
    if (this.#closed) return;
    if (attempt === 1) {
      this.#failInFlight(
        new UndraTransportError("closed", `the connection to the core was lost (${error.message}); reconnecting`, { cause: error }),
      );
    }
    this.#setConnection({ kind: "reconnecting", attempt, error });
  }

  /**
   * The connection is back: release what was released meanwhile and observe what the app observes
   * again. The core answers each `Observe` with the current values, so every mirror converges.
   */
  #reconnected(hello: HelloPayload): void {
    if (this.#closed) return;
    this.hello = hello;
    try {
      for (const handle of this.#releasedWhileDown) this.#transport.send(Kind.Release, encodeRelease({ handle }));
      this.#releasedWhileDown.clear();
      for (const [handle, signals] of this.#observed) {
        for (const signalId of signals) this.#transport.send(Kind.Observe, encodeObserve({ handle, signalId, on: true }));
      }
    } catch (error) {
      // The connection dropped again already: not connected after all. The transport reports the loss and the
      // next reconnect replays.
      this.#reportError("reconnect", error);
      return;
    }
    this.#setConnection({ kind: "connected" });
  }

  #setConnection(state: ConnectionState): void {
    this.#connection._set(state);
    this.#notifyConnection(state);
  }

  #notifyConnection(state: ConnectionState): void {
    try {
      this.#options.onConnectionChange?.(state);
    } catch (error) {
      this.#reportError("onConnectionChange", error);
    }
  }

  /** Asks the core for the current value of a signal the mirror lost (a dropped merged patch); the entries arrive as a change-set. */
  #resync(handle: Handle, signalId: number): void {
    if (this.#closed) return;
    try {
      this.#transport.send(Kind.Observe, encodeObserve({ handle, signalId, on: true }));
    } catch (error) {
      this.#reportError("resync", error);
    }
  }

  #allocCallId(): number {
    this.#nextCallId = nextCallId(this.#nextCallId, (id) => this.#pending.has(id));
    return this.#nextCallId;
  }

  #request(encode: (callId: number) => Uint8Array, signal?: AbortSignal): Promise<Uint8Array> {
    const call = this.#send(encode, signal);
    if (!this.#reporting) return call;
    // Started by the `onError` handler: it settles after the handler returned, out of reach of the
    // synchronous guard, so its failure is remembered and `report` only logs it.
    return call.catch((error: unknown) => {
      throw this.#fromHandler(error);
    });
  }

  /** `error`, the failure of a call the `onError` handler started, remembered as such. */
  #fromHandler(error: unknown): unknown {
    // A transport failure can be one object that every pending call shares (`close()` fails them all with
    // it): this call gets its own, so the other callers' reports are unaffected.
    const own =
      error instanceof UndraTransportError ? new UndraTransportError(error.reason, error.message, { cause: error }) : error;
    if (typeof own === "object" && own !== null) this.#handlerFailures.add(own);
    return own;
  }

  /** Whether `error` is the failure of a call the `onError` handler started. */
  #startedByHandler(error: unknown): boolean {
    return typeof error === "object" && error !== null && this.#handlerFailures.has(error);
  }

  #send(encode: (callId: number) => Uint8Array, signal?: AbortSignal): Promise<Uint8Array> {
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
          reject(this.#failureOf(error));
        }
      }
    });
  }

  #openStream(target: CallTargetArg, methodId: number, args: Uint8Array): StreamCall {
    const callId = this.#closed || this.#restarting ? 0 : this.#allocCallId();
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
    if (this.#closed || this.#restarting) {
      try {
        this.#assertOpen();
      } catch (error) {
        stream.fail(error);
      }
      return stream;
    }
    this.#pending.set(callId, { kind: "stream", stream });
    try {
      this.#transport.send(Kind.Call, encodeTarget(target, methodId, callId, args));
    } catch (error) {
      this.#pending.delete(callId);
      stream.fail(this.#failureOf(error));
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
      // The panic's own record, logged by the core before it traps: what the panic report says (ADR-046).
      if (level >= 5 && target === "undra::panic") this.#lastPanicRecord = message;
      this.#log(level, target, message);
    },
    closed: (error) => {
      this.#lost(error);
    },
    reconnecting: (attempt, error) => {
      this.#reconnecting(attempt, error);
    },
    reconnected: (hello) => {
      this.#reconnected(hello);
    },
    holdsObjects: () => this.#handles.size > 0,
    asyncPorts: () => this.#asyncPortIds(),
  };

  #onReply(payload: Uint8Array): void {
    if (payload.length < 5) {
      this.#reportError("reply", new UndraTransportError("protocol", "the core sent a truncated reply"));
      return;
    }
    const callId = new DataView(payload.buffer, payload.byteOffset, payload.byteLength).getUint32(0, true);
    const status = payload[4] as number;
    const body = payload.subarray(5);
    const entry = this.#pending.get(callId);
    if (entry === undefined) return; // aborted or cancelled meanwhile, or never ours

    if (status > ReplyStatus.BadRequest) {
      this.#pending.delete(callId);
      const error = new UndraTransportError("protocol", `the core sent reply status ${status}`);
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
          ? new UndraTransportError("protocol", "the core answered a stream call with a plain result")
          : new UndraReplyError(status as ReplyStatus, body),
      );
      return;
    }

    if (status === ReplyStatus.StreamOpened) {
      // A plain call to a stream method: free the stream the core just registered.
      this.#pending.delete(callId);
      entry.cleanup?.();
      entry.reject(new UndraError("state", "this method is a stream; call it with UndraCore.stream"));
      try {
        this.#transport.send(Kind.Cancel, encodeCancel({ callId }));
      } catch {
        // Closing anyway.
      }
      return;
    }
    this.#pending.delete(callId);
    entry.cleanup?.();
    // Read-your-writes: the change-sets that arrived before this reply are applied before the
    // caller's continuation runs (a microtask queued now runs before the one `resolve` queues).
    this.mirror.queueFlush();
    if (status === ReplyStatus.Ok) entry.resolve(body);
    else entry.reject(new UndraReplyError(status as ReplyStatus, body));
  }

  #onStreamItem(payload: Uint8Array): void {
    if (payload.length < 5) {
      this.#reportError("stream", new UndraTransportError("protocol", "the core sent a truncated stream item"));
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
        // The stream's own `E`; generated code decodes it.
        this.#pending.delete(callId);
        entry.stream.fail(new UndraReplyError(ReplyStatus.Error, body));
        return;
      case StreamFlag.Failed: {
        // Panicked, cancelled by the core or refused: exactly the failed reply with that status (ADR-036).
        this.#pending.delete(callId);
        let failure: StreamFailure;
        try {
          failure = decodeStreamFailure(body);
        } catch (error) {
          entry.stream.fail(
            new UndraTransportError("protocol", `the core sent a malformed stream failure: ${errorMessage(error)}`, {
              cause: error,
            }),
          );
          return;
        }
        entry.stream.fail(new UndraReplyError(failure.status, streamFailureReplyBody(failure)));
        return;
      }
      default:
        this.#pending.delete(callId);
        entry.stream.fail(new UndraTransportError("protocol", `the core sent stream flag ${flag}`));
    }
  }

  #onPortCall(call: PortCallPayload): PortOutcome {
    // A reply that settles after a restart belongs to a call of the instance that trapped: never deliver it to the new one.
    const epoch = this.#epoch;
    return dispatchPortCall(this.#ports.get(call.portId), call, {
      later: (reply) => {
        if (epoch === this.#epoch) this.#sendPortReply(reply);
      },
      untyped: (failed, error) => {
        this.#reportError(portOperation(failed), error);
      },
    });
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

  /** `report` for the runtime's own failures; `where` names the operation. */
  #reportError(where: string, error: unknown): void {
    this.report(error, where);
  }
}
