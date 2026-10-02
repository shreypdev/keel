import { lightAdapters } from "./adapters/browser-events.js";
import { UNNAMED_NAMESPACE, checkNamespace } from "./adapters/names.js";
import { defaultPorts } from "./adapters/default-ports.js";
import { startEventSources } from "./adapters/events.js";
import { WEB_CRYPTO_REQUIRED, consoleLog, hasCryptoRandom } from "./adapters/system.js";
import type { Adapters, AdapterOverrides, UndraBackgroundReport, UndraBackgroundStats, UndraPanicReport } from "./adapters/types.js";
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
import { isTrap } from "./panic.js";
import type { PanicReporter, PanicSupport } from "./panic-report.js";
import type { CrashRecovery, UndraCoreRestarted } from "./recovery.js";
import { errorMessage } from "./platform.js";
import type { PortImpl } from "./port.js";
import { dispatchPortCall, portOperation } from "./port-dispatch.js";
import { Signal } from "./signal.js";
import type { StreamSupport } from "./stream-support.js";
import type { ReconnectOptions, WebSocketFactory } from "./transport/remote.js";
import type { Channel, PortOutcome, Transport, TransportHandler } from "./transport/transport.js";
import { WasmHost, type WasmSource } from "./transport/wasm-main.js";
import type { WorkerLike } from "./transport/wasm-worker.js";
import {
  ALL_SIGNALS,
  CallTarget,
  type Handle,
  type HelloPayload,
  ReplyStatus,
  type PortCallPayload,
  codecs,
  decodeValue,
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
  | { readonly target: CallTarget.ObjectMethod; readonly handle: Handle }
  | PageTarget;

/** The target of a page call (ADR-043): the page server's handle and the rows wanted. */
type PageTarget = { readonly target: CallTarget.LazyListPage; readonly handle: Handle; readonly offset: number; readonly limit: number };

/** The argument type of `call`, `callSync` and `stream`. */
export type CallTargetArg = CallTargetRef | CallTarget.FreeFunction;

/** Live counters of a core; see {@link UndraCore.stats}. */
export interface UndraStats {
  /** Object handles alive in the core (its own count when it reports one, else the handles this runtime's wrappers hold). */
  readonly liveHandles: number;
  /**
   * References to objects the host owns, as the core counts them (`host_refs`, ADR-040): one per live wrapper, since a
   * reply that carries an object the host already wraps gives the extra reference back at once. Without the core's
   * count (over a socket), the handles this runtime's wrappers hold.
   */
  readonly hostRefs: number;
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
  /** Panics the core reported through the `Diagnostics` port (`panic_reports`); `0` for a core that does not count them, and for a wasm core (it traps instead of reporting). */
  readonly panicReports: number;
  /** What the core's background tasks have to do and have done (ADR-046 decision 3); every counter is `0` for a core that does not report them. */
  readonly background: UndraBackgroundStats;
}

/** How long the runtime lets the core drain its background work when a page goes to the background (ADR-046 decision 3.4), in ms. */
const PAGE_BACKGROUND_MS = 1000;

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
   * The namespace of the core (`UndraIds.namespace`, `[core] namespace` in undra.toml), which the default `Kv`, `SecureStore`,
   * `Fs` and `Db` stores are kept under: IndexedDB `undra.<namespace>.kv`, the origin-private-file-system directory
   * `undra/<namespace>/fs`, and so on (SPEC 8, ADR-044 amendment A), so two cores of one page never share a store. The
   * generated entry (`Undra<Namespace>.load`, `.attach`) fills it in. Default `"_"`, which two cores that are loaded without one
   * share, and a generated entry always sets one. It is the rule of `undra.toml`: lowercase letters, digits and `_`, starting
   * with a letter, at most 32; anything else (`..`, `a/b`, an empty one) is refused with `UndraError("options")` by `load` and
   * `attach`. An adapter you give (`adapters`, `ports`) keeps its own location. It is also the `namespace` of the panic report of a
   * wasm core that trapped (`onPanic`), which the module does not say itself: `""` for a core loaded without one.
   */
  readonly namespace?: string;
  /**
   * Adapters to use instead of the browser defaults (`browserAdapters()`, which the runtime builds piece by piece: the four port adapters on the first call to their port):
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
   * Restart a wasm core that trapped from its last snapshot (ADR-049), off by default: `recovery: crashRecovery()`, or
   * `crashRecovery({ snapshotEveryMs, maxSnapshotBytes, maxRestarts, perMs })` (one per core). The runtime keeps a
   * snapshot at most once a second while stores change (4 MiB at most, outside wasm memory, in the worker in
   * `wasm-worker` mode). On a trap: `onPanic` hears the panic; every call and stream in flight fails with
   * `UndraTransportError("restarted")` (it may or may not have run; it is not retried); the same compiled module is
   * instantiated again and the snapshot restored (stores keep their handles); every observed store is observed again;
   * query handles are re-created (their wrappers move to the new handles); then `onCoreRestarted` and `onError` hear an
   * `UndraCoreRestarted`. Store writes after the last snapshot, objects that are not stores (query handles excepted),
   * the core's running tasks and timers, and what the core held outside its stores are lost. One trap more than
   * `maxRestarts` within `perMs` (default 3 a minute) and the core stays dead: `onClose` reports the trap, as without
   * recovery. Wasm modes only (a native core contains its panics). The recovery code ships only with an app that
   * passes it.
   */
  readonly recovery?: CrashRecovery;
  /** Called after a wasm core trapped and was restarted (`recovery`), with what happened; `onError` receives the same value. */
  readonly onCoreRestarted?: (event: UndraCoreRestarted) => void;
  /**
   * Called once per panic of the core with its report (ADR-046 decision 4): the place to forward a core panic to a crash
   * reporter (Sentry, Crashlytics, `reportError`). A native core (React Native, a `remote` core) reports each panic it
   * contained through its `Diagnostics` port, on the JavaScript thread, in the order the panics happened; a wasm core traps, so
   * the runtime builds the same report from the core's FATAL `undra::panic` record and the trap's stack, once per trap,
   * before any restart (ADR-049) and with or without `recovery`. A handler that throws is reported to `onError` and changes
   * nothing. Without a handler a native core's report is logged at error level (one line: operation, message, location).
   * Setting it makes the runtime load the code that builds the report of a wasm trap, and hash the module (SHA-256) for
   * `imageId` while the module compiles: `load` resolves once both are there, so a trap right after it is reported in full.
   */
  readonly onPanic?: (report: UndraPanicReport) => void;
  /**
   * What to do when a web page goes to the background (ADR-046 decision 3.4, within the page's life only). The runtime reports
   * `Lifecycle.Background` when the page is hidden, left (`pagehide`) or frozen; the core then flushes its debounced persistence
   * at once, and if `stats().background.pending > 0` the runtime runs `runInBackground(1000)` without waiting for it (a failure
   * goes to `onError`). `false` turns the background run off (the Lifecycle report stays). Default on. Not for a core that is not in a page. Only the page's own life: Background Sync in a service worker would need the core to run inside the worker, and is a follow-up.
   */
  readonly backgroundRun?: boolean;
  /**
   * How the mirror delivers change-sets (docs/SPEC.md section 11): `schedule` replaces the frame
   * scheduler (`scheduleFrame`) that drains what the core produced on its own, and
   * `maxPendingEntries` (default 65,536) / `maxPendingBytes` (default 16 MiB) bound the backlog.
   */
  readonly mirror?: Pick<MirrorOptions, "schedule" | "maxPendingEntries" | "maxPendingBytes">;
  /**
   * **Development only, and inert unless the core is a `remote` one served by `undra dev`.** Called with a
   * one-line message the dev server says about itself, such as `Reloaded, state kept` after it rebuilt the
   * core (ADR-053): show it in a status bar for a few seconds. `undra dev` tells every client that attaches
   * soon after a rebuild, once; an in-process or production core never produces one, so the callback never
   * fires there. The message is also written to the log. The callback runs synchronously inside a core
   * callback: keep it short and do not call into Undra from it. An exception it throws is logged and dropped.
   */
  readonly onDevNotice?: (message: string) => void;
}

/** What a core keeps of the options it was started with: `load` has the rest of {@link LoadOptions}, `attach` does not. */
type CoreOptions = AttachOptions & Partial<Pick<LoadOptions, "wasm" | "coreVersion">>;

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
  /** The core's version, for the `coreVersion` of the panic report of a wasm trap: the module does not carry it, so default `""`. */
  readonly coreVersion?: string;
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

/** Statistics counters exposed by `undra_stats_json` that this runtime reads. */
type CoreStatsJson = Readonly<Record<string, unknown>>;

interface PendingCall {
  readonly kind: "call";
  resolve(body: Uint8Array): void;
  reject(error: unknown): void;
  cleanup: (() => void) | undefined;
}

/**
 * A call on a core that answers inside `send` (`wasm-main`): its reply is usually there before `send` returns, so it
 * is recorded here, with no promise, executor or closures built, and only a call that is still waiting after `send`
 * gets a promise (`UndraCore.call`).
 */
class DirectCall implements PendingCall {
  readonly kind = "call";
  cleanup: (() => void) | undefined = undefined;
  done = false;
  failed = false;
  value: unknown;
  resolve(body: Uint8Array): void {
    this.done = true;
    this.value = body;
  }
  reject(error: unknown): void {
    this.done = true;
    this.failed = true;
    this.value = error;
  }
}

/** A stream's entry in the pending map: the stream support (`stream-support.ts`) owns what happens to it. */
export interface PendingStream {
  readonly kind: "stream";
  /** The reply to the stream's call (`StreamOpened`, or its failure). */
  reply(status: number, body: Uint8Array): void;
  reject(error: unknown): void;
  cleanup?: undefined;
}

const UNLOADED_MESSAGE =
  "the core is not loaded: load it at app startup (the bindings' Undra<Namespace>.load(...), or UndraCore.load(...)), before creating any Undra object, or pass a core explicitly";
const DEFAULT_OBSERVE_TIMEOUT_MS = 10_000;

function abortReason(signal: AbortSignal): unknown {
  if (signal.reason !== undefined) return signal.reason;
  const error = new Error("The operation was aborted");
  error.name = "AbortError";
  return error;
}

/** The 32-bit halves of the last few handles used by calls: BigInt arithmetic allocates, and a store calls with the same handle again and again. */
const HANDLE_HALVES = 4;
const handleKeys: bigint[] = [];
const handleLo: number[] = [];
const handleHi: number[] = [];
let handleNext = 0;

/** Writes the `u64` `handle` at `out[at..at+8]` (little-endian): from the cache of recent handles, or after splitting it. */
function putHandle(out: Uint8Array, at: number, handle: Handle): void {
  let i = handleKeys.length;
  while (i-- > 0) if (handleKeys[i] === handle) break;
  if (i < 0) {
    if (BigInt.asUintN(64, handle) !== handle) throw new RangeError(`u64 out of range: ${String(handle)}`);
    i = handleNext;
    handleNext = (handleNext + 1) % HANDLE_HALVES;
    handleKeys[i] = handle;
    handleLo[i] = Number(handle & 0xffff_ffffn);
    handleHi[i] = Number(handle >> 32n);
  }
  put32(out, at, handleLo[i] as number);
  put32(out, at + 4, handleHi[i] as number);
}

function put32(out: Uint8Array, at: number, v: number): void {
  out[at] = v;
  out[at + 1] = v >>> 8;
  out[at + 2] = v >>> 16;
  out[at + 3] = v >>> 24;
}

/** The length of the `Call` header of a free function or a method (SPEC 3.3): target u8, handle u64, method id u32, call id u32. */
const HEAD_LEN = 17;

/** Writes that header into `out` (which holds at least {@link HEAD_LEN} bytes), clearing what an earlier call left in it. */
function writeHead(out: Uint8Array, target: CallTargetArg, methodId: number, callId: number): void {
  let handle: Handle | undefined;
  if (typeof target === "number") {
    if (target !== CallTarget.FreeFunction) {
      throw new TypeError("a bare CallTarget must be FreeFunction; pass { target, handle } for a method");
    }
  } else if (target.target === CallTarget.ObjectMethod) {
    handle = target.handle;
  }
  if (methodId >>> 0 !== methodId) throw new RangeError(`u32 out of range: ${String(methodId)}`);
  if (handle === undefined) {
    out.fill(0, 0, 9);
  } else {
    out[0] = CallTarget.ObjectMethod;
    putHandle(out, 1, handle);
  }
  put32(out, 9, methodId);
  put32(out, 13, callId);
}

/** A `Call` payload (SPEC 3.3) in one allocation: a free function or a method with its arguments, or a page call (no arguments, 21 bytes). */
function encodeTarget(target: CallTargetArg, methodId: number, callId: number, args: Uint8Array): Uint8Array {
  if ((target as CallTargetRef).target === CallTarget.LazyListPage) {
    const page = target as PageTarget;
    const out = new Uint8Array(21);
    out[0] = CallTarget.LazyListPage;
    putHandle(out, 1, page.handle);
    put32(out, 9, page.offset);
    put32(out, 13, page.limit);
    put32(out, 17, callId);
    return out;
  }
  const out = new Uint8Array(HEAD_LEN + args.length);
  writeHead(out, target, methodId, callId);
  if (args.length > 0) out.set(args, HEAD_LEN);
  return out;
}

/** The `Call` payload of a constructor (SPEC 3.3): target u8, type id u32, method id u32, call id u32, the arguments. */
function encodeConstructor(typeId: number, methodId: number, callId: number, args: Uint8Array): Uint8Array {
  const out = new Uint8Array(13 + args.length);
  out[0] = CallTarget.Constructor;
  put32(out, 1, typeId);
  put32(out, 5, methodId);
  put32(out, 9, callId);
  out.set(args, 13);
  return out;
}

/**
 * The reply body: `reply` past its 5-byte header (`call_id u32, status u8`). A small reply (what the core copied out of
 * its memory is a typed array V8 keeps on its heap up to 64 bytes) is copied: `subarray` would give it a backing store
 * of its own, which costs more than the copy.
 */
function replyBody(reply: Uint8Array): Uint8Array {
  return reply.length <= 64 ? reply.slice(5) : reply.subarray(5);
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
  private static _shared: UndraCore | null = null;

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
    return UndraCore._shared ?? UndraCore._placeholder();
  }

  /** The shared core, or `null` if none is loaded. While it is `null`, {@link UndraCore.shared} is the closed placeholder. */
  static get current(): UndraCore | null {
    return UndraCore._shared;
  }

  /**
   * The permanently closed placeholder: what {@link UndraCore.shared} returns while no core is loaded, and what
   * the generated entry of a core (`Undra<Namespace>.core`, ADR-044) returns while that core is not loaded. Its
   * calls reject with `UndraCallError.Unavailable` and its commands only log; its first use logs what to do.
   */
  static get unloaded(): UndraCore {
    return UndraCore._placeholder();
  }

  private static _unloaded: UndraCore | null = null;

  /** The placeholder `shared` returns while no core is loaded: a core that was closed from the start. */
  private static _placeholder(): UndraCore {
    if (UndraCore._unloaded === null) {
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
          sendCall: gone,
          observe: gone,
          release: gone,
          cancel: gone,
          streamCredit: gone,
          event: gone,
          timerFired: gone,
          portReply: gone,
        },
        { expectedSchemaHash: 0n, shared: false },
        {},
      );
      core._closed = true;
      core._closedMessage = UNLOADED_MESSAGE;
      UndraCore._unloaded = core;
      consoleLog().log(
        4,
        "undra::runtime",
        "a core was used while it is not loaded (before its load(...) succeeded, or after it was closed); calls on it reject with UndraCallError.Unavailable. Load the core at app startup, before creating any Undra object.",
      );
    }
    return UndraCore._unloaded;
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
   * random source (`wasm-main` only). A `namespace` that is not a core namespace (lowercase letters, digits and `_`, starting
   * with a letter, at most 32: `..`, `a/b`, an empty or a long one) rejects with `UndraError("options")` before anything is
   * created, because it names the default stores.
   */
  static async load(options: LoadOptions): Promise<UndraCore> {
    checkNamespace(options.namespace);
    const adapters = mergeAdapters(lightAdapters(), options.adapters);
    // The worker keeps the snapshots of a core in `wasm-worker` mode: it is told the policy (data, not code).
    const recovery = options.recovery?.options;
    let transport: Transport;
    switch (options.mode) {
      case "wasm-main": {
        if (options.wasm === undefined) throw new UndraError("options", "mode 'wasm-main' needs the `wasm` option");
        // The core's only random source is the `random` import: refuse before instantiating rather than let its
        // `Rng` fail at the first idempotency key (ADR-049). An app that supplies its own `rng` adapter has one.
        if (options.adapters?.rng == null && !hasCryptoRandom()) throw new UndraTransportError("unsupported", WEB_CRYPTO_REQUIRED);
        transport = new WasmHost({
          wasm: options.wasm,
          expectedSchemaHash: options.expectedSchemaHash,
          ...(options.platform !== undefined && { platform: options.platform }),
          ...(options.devtools !== undefined && { devtools: options.devtools }),
          ...(options.logLevel !== undefined && { logLevel: options.logLevel }),
          ...(adapters.clock && { clock: adapters.clock }),
          ...(adapters.rng && { rng: adapters.rng }),
          ...(adapters.timer && { timer: adapters.timer }),
          onError: (error) => {
            adapters.log?.log(4, "undra::runtime", `import failed: ${errorMessage(error)}`);
          },
        }) as unknown as Transport;
        break;
      }
      case "wasm-worker": {
        if (options.wasm === undefined) throw new UndraError("options", "mode 'wasm-worker' needs the `wasm` option");
        // Checked here, before the worker is spawned; the worker reads its own `crypto` (ADR-049).
        if (!hasCryptoRandom()) throw new UndraTransportError("unsupported", WEB_CRYPTO_REQUIRED);
        const { WasmWorkerTransport } = await import("./transport/wasm-worker.js");
        // A Worker (anything with `postMessage`) or a function creating one is `{ create }` in short.
        const worker = options.worker;
        const { create, ports } = (typeof worker === "object" && !("postMessage" in worker) ? worker : { create: worker }) as WorkerModeOptions;
        transport = new WasmWorkerTransport({
          wasm: options.wasm,
          expectedSchemaHash: options.expectedSchemaHash,
          ...(create && { worker: create }),
          ...(ports !== undefined && { ports }),
          ...(recovery && { recovery }),
          ...(options.platform !== undefined && { platform: options.platform }),
          ...(options.devtools !== undefined && { devtools: options.devtools }),
          ...(options.logLevel !== undefined && { logLevel: options.logLevel }),
          ...(options.handshakeTimeoutMs !== undefined && { startTimeoutMs: options.handshakeTimeoutMs }),
        });
        break;
      }
      case "remote": {
        if (options.url === undefined) throw new UndraError("options", "mode 'remote' needs the `url` option");
        // Fetched when an app asks for this mode (a development page served by `undra dev`, a native core over a socket), not by every page.
        const { RemoteTransport } = await import("./transport/remote.js");
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
    return UndraCore._attach(transport, options, adapters);
  }

  /**
   * Runs a core over a transport you provide (an embedder's IPC channel, a
   * test double) instead of one of the built-in modes. Everything else is as
   * for {@link UndraCore.load}, including the schema check on the transport's
   * `Hello`; a `namespace` that is not a core namespace rejects as it does for {@link UndraCore.load}.
   */
  static async attach(transport: Transport, options: AttachOptions): Promise<UndraCore> {
    checkNamespace(options.namespace);
    return UndraCore._attach(transport, options, mergeAdapters(lightAdapters(), options.adapters));
  }

  private static async _attach(transport: Transport, options: CoreOptions, adapters: Partial<Adapters>): Promise<UndraCore> {
    // A transport that only has `send(kind, payload)` (remote, worker, a custom one) is driven through the framed adapter.
    const framing = transport.observe === undefined || !transport.synchronous ? await import("./transport/framed.js") : undefined;
    const channel = transport.observe === undefined ? framing!.framed(transport) : (transport as Transport & Channel);
    const core = new UndraCore(channel, options, adapters);
    // A core that answers later resolves `observe` when the initial change-set was applied: the mirror's waiters.
    if (!transport.synchronous) core.mirror._w = framing!.mirrorWaiters(core.mirror);
    core._ext = framing?.extension;
    try {
      await core._start();
    } catch (error) {
      core._dispose(null);
      throw error;
    }
    if (options.shared !== false && UndraCore._shared === null) UndraCore._shared = core;
    return core;
  }

  /** The mirror that applies change-sets to stores; stores register themselves with it. */
  readonly mirror: Mirror;
  /** What the core said in its `Hello` (for wasm modes, synthesised from the module). Set once `load` resolves. */
  hello: HelloPayload = { undraVersion: "", schemaHash: 0n, platform: "", mode: "" };

  private readonly _transport: Transport & Channel;
  private readonly _options: CoreOptions;
  private readonly _adapters: Partial<Adapters>;
  private readonly _observeTimeoutMs: number;
  private readonly _ports = new Map<number, PortImpl>();
  private readonly _pending = new Map<number, PendingCall | PendingStream>();
  private readonly _handles = new Set<Handle>();
  /** The signals the app observes, per handle: what a reconnect observes again. */
  private readonly _observed = new Map<Handle, Set<number>>();
  /** @internal What a core that is not in this thread adds (`transport/framed.ts`): reconnects, its ports, the dev notice. */
  _ext: import("./transport/framed.js").CoreExtension | undefined;
  /** @internal How many times crash recovery restarted the core: a wrapper's finalizer compares it with the count at its birth. */
  _era = 0;
  private readonly _connection = new Signal<ConnectionState>({ kind: "connecting" });
  /** The header of the call being sent, reused: an in-process transport copies it before it returns (`Transport.sendCall`). */
  private readonly _head = new Uint8Array(HEAD_LEN);
  private _nextCallId = 0;
  private _closed = false;
  /** What a call on this closed core says; the default is "the core is closed". */
  private _closedMessage = "the core is closed";
  private _reporting = false;
  /** The failures of calls the `onError` handler started (see `report`): reported, they are only logged. */
  private readonly _handlerFailures = new WeakSet<object>();
  private _stopEvents: (() => void) | null = null;
  /** The message of the last FATAL `undra::panic` record: what a trap's panic report says (ADR-046). */
  private _lastPanicRecord: string | null = null;
  /** What reports the traps of this wasm core (`panic-report.ts`, loaded on demand: see {@link UndraCore._loadPanics}); `null` until it is there, and for a core that wants no reports. */
  private _panics: PanicReporter | null = null;
  /** A background run the page started is in flight. */
  private _backgroundRunning = false;

  private constructor(transport: Transport & Channel, options: CoreOptions, adapters: Partial<Adapters>) {
    this._options = options;
    this._adapters = adapters;
    // With `recovery`, the core runs over the layer that restarts it after a trap (ADR-049; `crashRecovery`).
    this._transport =
      (options.recovery?.attach(
        transport,
        {
          core: this,
          handles: this._handles,
          observed: this._observed,
          fail: (error) => this._failInFlight(error),
          lose: (error) => {
            this._lostForGood(error);
          },
          deliver: (error) => {
            this._hand(error);
          },
          panicked: (trap) => this._panicReport(trap),
          ports: this._ports,
        },
        options.onCoreRestarted,
      ) as (Transport & Channel) | undefined) ?? transport;
    this._notifyConnection(this._connection.peek());
    this._observeTimeoutMs = options.observeTimeoutMs ?? DEFAULT_OBSERVE_TIMEOUT_MS;
    this.mirror = new Mirror({
      ...options.mirror,
      onError: (error) => {
        this._reportError("mirror", error);
      },
      resync: (handle, signalId) => {
        this._resync(handle, signalId);
      },
    });
    for (const [portId, impl] of defaultPorts(options.adapters, options.namespace)) this._ports.set(portId, impl);
    if (options.ports !== undefined) {
      for (const [portId, impl] of Object.entries(options.ports)) {
        this._ports.set(Number(portId), impl);
        impl.bind?.({ namespace: this.namespace });
      }
    }
  }

  /**
   * The namespace of the core: the one its generated entry loaded it under ({@link AttachOptions.namespace}), `"_"`
   * when it was loaded without one. The default stores are kept under it (ADR-044 amendment A).
   */
  get namespace(): string {
    return this._options.namespace ?? UNNAMED_NAMESPACE;
  }

  /** The mode of the transport (`"wasm-main"`, `"wasm-worker"`, `"remote"`, or a custom one). */
  get mode(): string {
    return this._transport.mode;
  }

  /** Whether the core has been closed, by `close()` or because the channel was lost. */
  get closed(): boolean {
    return this._closed;
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
    return this._connection;
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
    const transport = this._transport;
    if (transport.callSync === undefined) throw new UndraModeError("callSync", transport.mode);
    this._assertOpen();
    let reply: Uint8Array;
    if (transport.callSyncParts === undefined || (target as CallTargetRef).target === CallTarget.LazyListPage) {
      reply = transport.callSync(encodeTarget(target, methodId, this._allocCallId(), args));
    } else {
      writeHead(this._head, target, methodId, this._allocCallId());
      reply = transport.callSyncParts(this._head, args);
    }
    // Read-your-writes (docs/SPEC.md section 11): the call's change-sets are queued by now.
    this.mirror.flush();
    if (reply.length < 5) throw new UndraTransportError("protocol", "the core returned a truncated reply");
    const status = reply[4] as number;
    const body = replyBody(reply);
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
   *
   * `orphan` is for a method whose reply carries references the host owns (objects, ADR-040): when the caller aborts
   * after the core answered (on a transport where the reply is still on its way), the reply that arrives for the
   * abandoned call is handed to it, which gives them back; without one it is dropped.
   */
  call(target: CallTargetArg, methodId: number, args: Uint8Array, signal?: AbortSignal, orphan?: (body: Uint8Array) => void): Promise<Uint8Array> {
    if (signal === undefined) {
      if (this._transport.synchronous && !this._reporting && !this._closed) return this._callDirect(target, methodId, args);
    } else if (signal.aborted) {
      return Promise.reject(abortReason(signal));
    }
    return this._request((callId) => encodeTarget(target, methodId, callId, args), signal, orphan);
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
    const core = this;
    return {
      [Symbol.asyncIterator]: () =>
        core._streams?.open(core, target, methodId, args) ??
        // No `features: [streams]` (a core loaded without its generated entry): the support loads on the first stream.
        (async function* () {
          yield* (core._streams = (await import("./stream-support.js")).streams).open(core, target, methodId, args);
        })(),
    };
  }

  /** @internal The stream support, once a feature or the first stream installed it. */
  _streams: StreamSupport | undefined;
  /** @internal For the stream support: the pending map, the transport, a fresh call id, the request encoder. */
  get _internals() {
    return {
      pending: this._pending,
      transport: this._transport,
      callId: () => this._allocCallId(),
      encode: encodeTarget,
      closedMessage: this._closedMessage,
      observed: this._observed,
      failInFlight: (error: Error) => this._failInFlight(error),
      setConnection: (state: ConnectionState) => this._setConnection(state),
      log: (level: number, target: string, message: string) => this._log(level, target, message),
    };
  }

  /**
   * Runs a constructor (`typeId` names the object type, `methodId` the
   * constructor) and resolves with the new object's handle. Rejects like `call`,
   * and with {@link UndraTransportError} (`"protocol"`) when the core answers
   * with the null handle.
   */
  async construct(typeId: number, methodId: number, args: Uint8Array): Promise<Handle> {
    const body = await this._request((callId) => encodeConstructor(typeId, methodId, callId, args));
    const handle = decodeValue(codecs.u64, body);
    if (handle === 0n) throw new UndraTransportError("protocol", "the core returned the null handle for a constructor");
    this._handles.add(handle);
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
      this._assertOpen();
      this._transport.observe(handle, signalId, on);
    } catch (error) {
      return Promise.reject(error);
    }
    this._noteObserved(handle, signalId, on);
    if (this._transport.synchronous) {
      // The core has already delivered the initial change-set; apply it now.
      this.mirror.flush();
      return Promise.resolve();
    }
    return on ? this.mirror._w!.when(handle, signalId, this._observeTimeoutMs) : Promise.resolve();
  }

  /** Releases an object handle: the store is unregistered from the mirror and the core drops its reference. Unknown handles and a closed core are ignored. */
  release(handle: Handle): void {
    this.mirror.unregister(handle);
    this._handles.delete(handle);
    this._observed.delete(handle);
    this._giveBack(handle);
  }

  /**
   * Gives one reference to `handle` back to the core without touching the wrapper that holds the handle: what `adopt`
   * does with a reply's reference to an object the host already wraps (ADR-040). Ignored by a closed core.
   *
   * @internal Called by `adopt` and `release`.
   */
  _giveBack(handle: Handle): void {
    if (this._closed) return;
    // While a `remote` core reconnects it keeps the object for us (ADR-051); the extension releases it when the connection is back.
    if (this._ext?.held(this, handle)) return;
    try {
      this._transport.release(handle);
    } catch (error) {
      this._reportError("release", error);
    }
  }

  /**
   * Counts `handle` among the handles this runtime's wrappers hold (a reply's object that a new wrapper now owns, ADR-040),
   * as `construct` does for a constructor's.
   *
   * @internal Called by `adopt`.
   */
  _held(handle: Handle): void {
    this._handles.add(handle);
  }

  /** Remembers what the app observes, so that a reconnect can observe it again. */
  private _noteObserved(handle: Handle, signalId: number, on: boolean): void {
    if (on) {
      let signals = this._observed.get(handle);
      if (signals === undefined) this._observed.set(handle, (signals = new Set()));
      signals.add(signalId);
    } else if (signalId === ALL_SIGNALS) {
      this._observed.delete(handle);
    } else {
      const signals = this._observed.get(handle);
      signals?.delete(signalId);
      if (signals?.size === 0) this._observed.delete(handle);
    }
  }

  /** Sends a host-to-core event of an event port (`Connectivity.changed`, `Lifecycle.changed`, ...). Throws {@link UndraTransportError} when the core is closed. */
  event(portId: number, methodId: number, payload: Uint8Array): void {
    this._assertOpen();
    this._transport.event(portId, methodId, payload);
  }

  /** Tells the core that a timer it set through a foreign `Timer` port is due (see `timerPort`). Wasm cores own their timers and do not need this. */
  timerFired(timerId: number): void {
    this._assertOpen();
    this._transport.timerFired(timerId);
  }

  /**
   * Registers the host implementation of a core port (SPEC 17.1), replacing
   * any earlier one for `portId`. Generated code offers `<name>PortImpl`
   * adapters that build the {@link PortImpl} from a typed implementation.
   *
   * In `wasm-worker` mode a synchronous port (`impl.sync`) cannot be served from
   * this thread, because the core cannot wait for it: registering one throws
   * `UndraError("options")` naming the port and the fix (register it in the
   * worker, in the module of `LoadOptions.worker.ports`, ADR-049; or load the
   * core in mode `"wasm-main"`). An asynchronous port registered after load is
   * announced to the worker.
   */
  registerPort(portId: number, impl: PortImpl): void {
    this._transport.portAdded?.(portId, impl);
    this._ports.set(portId, impl);
    impl.bind?.({ namespace: this.namespace });
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
    if (this._ext?.down(this, unhandled.error)) {
      this._log(3, "undra::runtime", `${unhandled.message} (the connection to the core is down: see UndraCore.connection)`);
      return;
    }
    this._log(4, "undra::runtime", unhandled.message);
    if (!this._startedByHandler(error)) this._hand(unhandled);
  }

  /** Calls `onError` with `unhandled`, unless it is running already; what it throws is logged. */
  private _hand(unhandled: UndraUnhandledError): void {
    const handler = this._options.onError;
    if (handler === undefined || this._reporting) return;
    this._reporting = true;
    try {
      handler(unhandled);
    } catch (thrown) {
      this._log(4, "undra::runtime", `the onError handler threw while handling "${unhandled.message}": ${errorMessage(thrown)}`);
    } finally {
      this._reporting = false;
    }
  }

  /** Live counters of this core; see {@link UndraStats}. */
  async stats(): Promise<UndraStats> {
    let core: CoreStatsJson | null = null;
    const json = this._closed ? null : await this._transport.stats?.().catch(() => null);
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
    for (const p of this._pending.values()) {
      if (p.kind === "call") calls++;
      else streams++;
    }
    const coreHandles = core?.live_handles;
    const count = (value: unknown): number => (typeof value === "number" ? value : 0);
    const background = (core?.background ?? {}) as CoreStatsJson;
    const hostRefs = core?.host_refs;
    return {
      liveHandles: typeof coreHandles === "number" ? coreHandles : this._handles.size,
      hostRefs: typeof hostRefs === "number" ? hostRefs : this._handles.size,
      pendingCalls: calls,
      openStreams: streams,
      mirroredStores: this.mirror.size,
      droppedEntries: this.mirror.dropped,
      mirror: this.mirror.stats(),
      core,
      panicReports: count(core?.panic_reports),
      background: {
        tasks: count(background.tasks),
        pending: count(background.pending),
        runs: count(background.runs),
        finished: count(background.finished),
        replayed: count(background.replayed),
        refetched: count(background.refetched),
      },
    };
  }

  /**
   * Gives the core a window to drain its background work (ADR-046 decision 3): replays the offline queue, fetches stale
   * persisted queries again and flushes pending persistence, concurrently, and resolves when every task finished or
   * `deadlineMs` less half a second (kept for the host) has passed, whichever is first. Over a native core this is what an OS
   * background task (a `BGTaskScheduler` task, WorkManager) calls; in a page the runtime calls it itself when the page goes
   * to the background (see `AttachOptions.backgroundRun`). `stats().background.pending` says whether there is anything to drain.
   *
   * Aborting `signal` cancels the call in the core: the work already done is kept (the offline queue persists per item) and the
   * promise rejects with the signal's reason. Never rejects with a panic: the failures are `UndraCallError`s (`Unavailable` for a
   * closed core, `CancelledByCore` when the core ended the call).
   *
   * ```ts
   * const report = await core.runInBackground(25_000, { signal });
   * if (!report.finished) scheduleAnotherWindow(report.stillPending);
   * ```
   */
  async runInBackground(deadlineMs: number, options: { readonly signal?: AbortSignal } = {}): Promise<UndraBackgroundReport> {
    try {
      // Loaded on demand (`background.ts`): a hello page, whose core has no background task, never runs it (ADR-052).
      return await (await import("./background.js")).runInBackground(this, deadlineMs, options.signal);
    } catch (error) {
      throw UndraCallError.mapped(error);
    }
  }

  /**
   * The persisted state of every store (docs/SPEC.md section 5.9), as opaque bytes that `restore`
   * accepts, also into a core loaded later from the same module. Objects that are not stores are
   * not part of it. In `wasm-worker` mode the snapshot is taken by the worker behind the messages
   * sent before it. Rejects with {@link UndraTransportError} when the core is closed, and with
   * {@link UndraModeError} over a transport that cannot snapshot (`remote`).
   */
  async snapshot(): Promise<Uint8Array> {
    this._assertOpen();
    const transport = this._transport;
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
    this._assertOpen();
    const transport = this._transport;
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
    this._dispose(new UndraTransportError("closed", "the core was closed"));
  }

  // ----- internals -------------------------------------------------------------------

  private async _start(): Promise<void> {
    const reporterLoaded = this._loadPanics();
    // What only a core that is not in this thread needs before and after its Hello (`transport/framed.ts`).
    const started = await this._ext?.starting(this, this._options, this._adapters, this._ports);
    const hello = await this._transport.start(this._handler);
    if (hello.schemaHash !== this._options.expectedSchemaHash) {
      throw new UndraSchemaMismatchError(this._options.expectedSchemaHash, hello.schemaHash);
    }
    // The report builder was fetched while the core loaded: it is there before the first call, so that no trap finds it missing.
    await reporterLoaded;
    this.hello = hello;
    this._setConnection({ kind: "connected" });
    started?.();
    this._stopEvents = startEventSources(
      this,
      this._adapters,
      (error) => {
        this._reportError("event", error);
      },
      () => {
        this._backgroundWindow();
      },
    );
  }

  /**
   * The core was told it is in the background (`Lifecycle.Background`). A web page then drains what the core has queued, inside the
   * time the browser still gives it (ADR-046 decision 3.4; `backgroundRun: false` and anything but a page do not): if there is work a
   * background window would drain, runs `runInBackground(1000)` and does not wait for it. One at a time; a failure goes to `onError`
   * (not when the core is closed: the app did that).
   */
  private _backgroundWindow(): void {
    if (this._backgroundRunning || this._options.backgroundRun === false || typeof document === "undefined") return;
    this._backgroundRunning = true;
    this.stats()
      .then((stats) => (stats.background.pending > 0 ? this.runInBackground(PAGE_BACKGROUND_MS) : undefined))
      .catch((error: unknown) => {
        if (!this._closed) this._reportError("runInBackground", error);
      })
      .finally(() => {
        this._backgroundRunning = false;
      });
  }

  /**
   * Gets ready to report a trap of a wasm core (ADR-046 decision 4.4) when the app wants reports: loads the code that builds
   * them (a page without `onPanic` or `recovery` never fetches it; with `recovery` it came with it) and, with `onPanic`, hashes
   * the module for `imageId`; `_start` waits for both while the module is fetched, so that they are there when a trap needs them.
   */
  private _loadPanics(): Promise<void> | undefined {
    const options = this._options;
    if (!this._transport.mode.startsWith("wasm") || (options.onPanic === undefined && options.recovery === undefined)) return undefined;
    // Ready when the builder is there and the module is hashed (`imageId`): `_start` waits for both.
    const start = (support: PanicSupport): Promise<void> => (this._panics = support.start(this, options)).ready;
    if (options.recovery !== undefined) return start(options.recovery.panics);
    return import("./panic-report.js").then((module) => start(module.panicSupport), (error: unknown) => this._reportError("onPanic", error));
  }

  private _assertOpen(): void {
    if (this._closed) throw new UndraTransportError("closed", this._closedMessage);
  }


  /** Stops everything. `reason` is what pending work fails with; `null` when there is none (a failed start). */
  private _dispose(reason: Error | null, why: ConnectionClosedReason = reason === null ? "failed" : "requested"): void {
    if (this._closed) return;
    this._closed = true;
    if (UndraCore._shared === this) UndraCore._shared = null;
    this._stopEvents?.();
    this._stopEvents = null;
    this._failInFlight(reason ?? new UndraTransportError("closed", "the core is closed"));
    this._setConnection(reason === null || why === "requested" ? { kind: "closed", reason: why } : { kind: "closed", reason: why, error: reason });
    this._transport.close();
    // Ports that hold platform resources for the core (the opt-in bindings) release them; `dispose` must not throw.
    for (const impl of this._ports.values()) impl.dispose?.();
  }

  /** Fails every call, stream and `observe` that waits for the core with `failure`; returns how many calls and streams there were. */
  private _failInFlight(failure: Error): number {
    const pending = [...this._pending.values()];
    this._pending.clear();
    for (const p of pending) {
      p.cleanup?.();
      p.reject(failure);
    }
    this.mirror.failWaiters(failure);
    return pending.length;
  }

  /** The channel to the core was lost; a trap's panic report goes to `onPanic` first. (A trap the core recovers from never gets here: `crashRecovery` restarts it.) */
  private _lost(error: Error): void {
    if (this._closed) return;
    if (isTrap(error) && this._panics !== null) this._panicReport(error);
    this._lostForGood(error);
  }

  /** The channel to the core is lost for good: the core closes and `onClose` hears why. */
  private _lostForGood(error: Error): void {
    if (this._closed) return;
    const why: ConnectionClosedReason =
      error instanceof UndraSchemaMismatchError ? "schemaMismatch" : error instanceof UndraSessionLostError ? "sessionLost" : "failed";
    this._dispose(error, why);
    try {
      this._options.onClose?.(error);
    } catch (thrown) {
      this._reportError("onClose", thrown);
    }
  }

  // ----- recovery (ADR-049 decision 3; the restart sequence is `crashRecovery`'s) ---------------

  /**
   * Registers a store the runtime re-creates after a restart instead of restoring it (a query handle): its constructor
   * call is recorded, and after a restart it runs again and the store moves to the new handle.
   *
   * @internal Called by `UndraStore` for the `recreate` option that generated query handles pass.
   */
  _recreatable(store: UndraStore, call: RecreateCall): void {
    this._options.recovery?.track(store, call);
  }

  /** The panic report of `trap` (ADR-046 decision 4.4), handed to `onPanic`. The builder is loaded: see {@link UndraCore._loadPanics}. */
  private _panicReport(trap: Error): UndraPanicReport {
    const report = (this._panics as PanicReporter).trapped(this._lastPanicRecord, trap);
    this._lastPanicRecord = null;
    return report;
  }

  private _setConnection(state: ConnectionState): void {
    this._connection._set(state);
    this._notifyConnection(state);
  }

  private _notifyConnection(state: ConnectionState): void {
    try {
      this._options.onConnectionChange?.(state);
    } catch (error) {
      this._reportError("onConnectionChange", error);
    }
  }

  /** Asks the core for the current value of a signal the mirror lost (a dropped merged patch); the entries arrive as a change-set. */
  private _resync(handle: Handle, signalId: number): void {
    if (this._closed) return;
    try {
      this._transport.observe(handle, signalId, true);
    } catch (error) {
      this._reportError("resync", error);
    }
  }

  private _allocCallId(): number {
    this._nextCallId = nextCallId(this._nextCallId, (id) => this._pending.has(id));
    return this._nextCallId;
  }

  private _request(encode: (callId: number) => Uint8Array, signal?: AbortSignal, orphan?: (body: Uint8Array) => void): Promise<Uint8Array> {
    const call = this._send(encode, signal, orphan);
    if (!this._reporting) return call;
    // Started by the `onError` handler: it settles after the handler returned, out of reach of the
    // synchronous guard, so its failure is remembered and `report` only logs it.
    return call.catch((error: unknown) => {
      throw this._fromHandler(error);
    });
  }

  /** `error`, the failure of a call the `onError` handler started, remembered as such. */
  private _fromHandler(error: unknown): unknown {
    // A transport failure can be one object that every pending call shares (`close()` fails them all with
    // it): this call gets its own, so the other callers' reports are unaffected.
    const own =
      error instanceof UndraTransportError ? new UndraTransportError(error.reason, error.message, { cause: error }) : error;
    if (typeof own === "object" && own !== null) this._handlerFailures.add(own);
    return own;
  }

  /** Whether `error` is the failure of a call the `onError` handler started. */
  private _startedByHandler(error: unknown): boolean {
    return typeof error === "object" && error !== null && this._handlerFailures.has(error);
  }

  private _send(encode: (callId: number) => Uint8Array, signal?: AbortSignal, orphan?: (body: Uint8Array) => void): Promise<Uint8Array> {
    try {
      this._assertOpen();
    } catch (error) {
      return Promise.reject(error);
    }
    const callId = this._allocCallId();
    return new Promise<Uint8Array>((resolve, reject) => {
      const entry: PendingCall = { kind: "call", resolve, reject, cleanup: undefined };
      this._pending.set(callId, entry);
      if (signal !== undefined) {
        const onAbort = (): void => {
          if (this._pending.get(callId) !== entry) return;
          // Before the cancel goes out: a transport that answers a cancel inside the send (`wasm-main`) must find
          // the call already abandoned, so the caller sees its abort reason and not the core's `CancelledByCore`:
          // the promise is settled first, so any later answer finds it so (rejecting it again does nothing).
          reject(abortReason(signal));
          // The core may have answered before it saw the cancel: that reply, if it is a success, still carries
          // references the host owns. It is the entry's answer, and `orphan` gives them back.
          if (orphan === undefined) this._pending.delete(callId);
          else entry.resolve = orphan;
          try {
            this._transport.cancel(callId);
          } catch {
            // The channel is gone; the core cancels with it.
          }
        };
        signal.addEventListener("abort", onAbort, { once: true });
        entry.cleanup = () => {
          signal.removeEventListener("abort", onAbort);
        };
      }
      try {
        this._transport.sendCall(encode(callId));
      } catch (error) {
        if (this._pending.get(callId) === entry) {
          this._pending.delete(callId);
          entry.cleanup?.();
          reject(error);
        }
      }
    });
  }

  /**
   * `call` for a core that answers inside `send`, without a signal: the reply to a synchronous method is recorded by
   * `_onReply` before `send` returns, and the promise handed back is already settled; a call the core answers later
   * (an asynchronous method) gets its promise after `send`, which is before anything can reply (`undra_poll` runs from
   * a microtask). The change-sets that arrived before the reply are still applied before the caller resumes.
   */
  private _callDirect(target: CallTargetArg, methodId: number, args: Uint8Array): Promise<Uint8Array> {
    const callId = this._allocCallId();
    const entry = new DirectCall();
    this._pending.set(callId, entry);
    try {
      if ((target as CallTargetRef).target === CallTarget.LazyListPage) {
        this._transport.sendCall(encodeTarget(target, methodId, callId, args));
      } else {
        writeHead(this._head, target, methodId, callId);
        this._transport.sendCall(this._head, args);
      }
    } catch (error) {
      // A send that fails before the core answered fails the call. One that fails after (a trap in the same export, the
      // reply already delivered) leaves the answer standing, as it does for a call that has a promise (`_send`).
      if (this._pending.get(callId) === entry) {
        this._pending.delete(callId);
        return Promise.reject(error);
      }
      if (!entry.done) return Promise.reject(error);
    }
    if (entry.done) return entry.failed ? Promise.reject(entry.value) : Promise.resolve(entry.value as Uint8Array);
    return new Promise<Uint8Array>((resolve, reject) => {
      entry.resolve = resolve;
      entry.reject = reject;
    });
  }

  // ----- what the transport tells us --------------------------------------------------

  private readonly _handler: TransportHandler = {
    reply: (payload) => {
      this._onReply(payload);
    },
    changeSet: (payload) => {
      this.mirror.enqueue(payload);
    },
    streamItem: (payload) => {
      this._streams?.item(this, payload);
    },
    portCall: (call) => this._onPortCall(call),
    log: (level, target, message) => {
      // The panic's own record, logged by the core before it traps: what the panic report says (ADR-046).
      if (level >= 5 && target === "undra::panic") this._lastPanicRecord = message;
      this._log(level, target, message);
      this._ext?.logged(this, this._options, target, message);
    },
    closed: (error) => {
      this._lost(error);
    },
    reconnecting: (attempt, error) => {
      this._ext?.reconnecting(this, attempt, error);
    },
    reconnected: (hello) => {
      this._ext?.reconnected(this, hello);
    },
    holdsObjects: () => this._handles.size > 0,
    ports: () => this._ports,
  };

  private _onReply(payload: Uint8Array): void {
    if (payload.length < 5) {
      this._reportError("reply", new UndraTransportError("protocol", "the core sent a truncated reply"));
      return;
    }
    const callId = ((payload[0] as number) | ((payload[1] as number) << 8) | ((payload[2] as number) << 16) | ((payload[3] as number) << 24)) >>> 0;
    const status = payload[4] as number;
    const body = replyBody(payload);
    const entry = this._pending.get(callId);
    if (entry === undefined) return; // aborted or cancelled meanwhile, or never ours

    if (entry.kind === "stream") {
      entry.reply(status, body);
      return;
    }
    if (status > ReplyStatus.BadRequest) {
      this._pending.delete(callId);
      entry.cleanup?.();
      entry.reject(new UndraTransportError("protocol", `the core sent reply status ${status}`));
      return;
    }

    if (status === ReplyStatus.StreamOpened) {
      // A plain call to a stream method: free the stream the core just registered.
      this._pending.delete(callId);
      entry.cleanup?.();
      entry.reject(new UndraError("state", "this method is a stream; call it with UndraCore.stream"));
      try {
        this._transport.cancel(callId);
      } catch {
        // Closing anyway.
      }
      return;
    }
    this._pending.delete(callId);
    entry.cleanup?.();
    // Read-your-writes: the change-sets that arrived before this reply are applied before the
    // caller's continuation runs (a microtask queued now runs before the one `resolve` queues).
    this.mirror.queueFlush();
    if (status === ReplyStatus.Ok) entry.resolve(body);
    else entry.reject(new UndraReplyError(status as ReplyStatus, body));
  }

  private _onPortCall(call: PortCallPayload): PortOutcome {
    const impl = this._ports.get(call.portId);
    return dispatchPortCall(impl, call, {
      later: (reply) => {
        this._sendPortReply(reply);
      },
      untyped: (failed, error) => {
        this._reportError(portOperation(failed, impl), error);
      },
    });
  }

  private _sendPortReply(reply: Uint8Array): void {
    if (this._closed) return;
    try {
      this._transport.portReply(reply);
    } catch (error) {
      this._reportError("port reply", error);
    }
  }

  private _log(level: number, target: string, message: string): void {
    try {
      (this._adapters.log ?? consoleLog()).log(level, target, message);
    } catch {
      // A failing log sink must not break the core.
    }
  }

  /** `report` for the runtime's own failures; `where` names the operation. */
  private _reportError(where: string, error: unknown): void {
    this.report(error, where);
  }
}
