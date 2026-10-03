import { UndraCallError, UndraUnhandledError } from "./call-error.js";
import type { UndraCore } from "./core.js";
import { UndraTransportError } from "./errors.js";
import { UndraRestoreError } from "./errors-rare.js";
import { wrapperOf } from "./identity.js";
import type { UndraPanicFrame, UndraPanicReport } from "./adapters/types.js";
import { isTrap } from "./panic.js";
import { type PanicSupport, panicSupport } from "./panic-report.js";
import type { PortImpl } from "./port.js";
import { errorMessage } from "./platform.js";
import type { CoreTransport, Transport, TransportHandler } from "./transport/transport.js";
import { WasmHost } from "./transport/wasm-main.js";
import { restoreInto, takeSnapshot, twin } from "./transport/wasm-snapshot.js";
import { type Handle, type HelloPayload, decodeSnapshot, encodeSnapshot, handleGeneration } from "./wire/index.js";
import { msg } from "./messages.js";

/*
 * Recovering a web core that trapped (ADR-049 decision 3): `crashRecovery(options)`, which `LoadOptions.recovery`
 * takes, and everything it brings: the snapshot keeper that runs where the core runs (the main thread in `wasm-main`,
 * the worker in `wasm-worker`), the restart sequence, and the event an app hears after a restart. None of it is part
 * of a core loaded without `recovery`: an app that does not ask for it does not ship it (ADR-052's size gate).
 */

/** What `crashRecovery(options)` takes: every field is optional. */
export interface RecoveryOptions {
  /** At most one snapshot per this many ms, taken after the core changed a store. Default 1000. */
  readonly snapshotEveryMs?: number;
  /** A snapshot larger than this is not kept (the previous one stays) and is reported once. Default 4 MiB. */
  readonly maxSnapshotBytes?: number;
  /** At most this many restarts within `perMs`; one more trap and the core stays dead (`onClose`). Default 3. */
  readonly maxRestarts?: number;
  /** The window of `maxRestarts`, in ms. Default 60,000. */
  readonly perMs?: number;
}

/** {@link RecoveryOptions} with every default applied. */
export interface ResolvedRecovery {
  /** At most one snapshot per this many ms. */
  readonly snapshotEveryMs: number;
  /** The largest snapshot kept, in bytes. */
  readonly maxSnapshotBytes: number;
  /** Restarts allowed within `perMs`. */
  readonly maxRestarts: number;
  /** The window of `maxRestarts`, in ms. */
  readonly perMs: number;
}

/** The defaults of {@link RecoveryOptions} (ADR-049). */
export const DEFAULT_RECOVERY: ResolvedRecovery = Object.freeze({
  snapshotEveryMs: 1000,
  maxSnapshotBytes: 4 * 1024 * 1024,
  maxRestarts: 3,
  perMs: 60_000,
});

/** `options` with every default applied. */
export function resolveRecovery(given: RecoveryOptions = {}): ResolvedRecovery {
  const pick = (value: number | undefined, fallback: number, min: number): number =>
    value === undefined || !Number.isFinite(value) ? fallback : Math.max(min, Math.floor(value));
  return {
    snapshotEveryMs: pick(given.snapshotEveryMs, DEFAULT_RECOVERY.snapshotEveryMs, 0),
    maxSnapshotBytes: pick(given.maxSnapshotBytes, DEFAULT_RECOVERY.maxSnapshotBytes, 0),
    maxRestarts: pick(given.maxRestarts, DEFAULT_RECOVERY.maxRestarts, 0),
    perMs: pick(given.perMs, DEFAULT_RECOVERY.perMs, 1),
  };
}

/** The part of {@link ResolvedRecovery} the side that runs the core needs (it travels to the worker in `init`). */
export interface SnapshotPolicy {
  /** At most one snapshot per this many ms. */
  readonly snapshotEveryMs: number;
  /** A snapshot larger than this many bytes is not kept. */
  readonly maxSnapshotBytes: number;
}

/** The last snapshot a {@link SnapshotKeeper} kept: a copy outside wasm memory, and when it was taken (`Date.now`). */
export interface KeptSnapshot {
  readonly data: ArrayBuffer;
  readonly takenAt: number;
}

/** What a {@link SnapshotKeeper} needs from the transport that runs the core. */
export interface SnapshotSource {
  /** `undra_snapshot`, copied out of wasm memory; throws when the core cannot be asked now (closed, trapped). */
  take(): Uint8Array;
  /** Reports a snapshot that was not kept because it is too large (called once). */
  tooLarge(bytes: number, limit: number): void;
  /** Reports a snapshot that failed. */
  failed(error: unknown): void;
}

type Idle = (callback: () => void, options?: { timeout: number }) => number;

/**
 * Keeps the last snapshot of a core for a restart (ADR-049 decision 3.3): after the core changed a store
 * (`changed()`, called when it emits a change-set), a snapshot is taken at most once per `snapshotEveryMs`,
 * in an idle callback where the platform has `requestIdleCallback`, and kept as an `ArrayBuffer`. One larger
 * than `maxSnapshotBytes` is not kept (the previous one stays) and is reported once.
 */
export class SnapshotKeeper {
  readonly #policy: SnapshotPolicy;
  readonly #source: SnapshotSource;
  readonly #now: () => number;
  #last: KeptSnapshot | null = null;
  #lastAttempt = Number.NEGATIVE_INFINITY;
  #scheduled = false;
  #dirty = false;
  #stopped = false;
  #paused = false;
  #toldTooLarge = false;
  #timer: ReturnType<typeof setTimeout> | undefined;

  /** @param policy How often and how large. @param source The transport's side. @param now The clock; default `Date.now`. */
  constructor(policy: SnapshotPolicy, source: SnapshotSource, now: () => number = Date.now) {
    this.#policy = policy;
    this.#source = source;
    this.#now = now;
  }

  /** The last snapshot kept, or `null` before the first. */
  get last(): KeptSnapshot | null {
    return this.#last;
  }

  /** The core changed a store: a snapshot is due (no sooner than `snapshotEveryMs` after the last one). */
  changed(): void {
    if (this.#stopped) return;
    this.#dirty = true;
    if (this.#scheduled) return;
    this.#scheduled = true;
    const wait = Math.max(0, this.#lastAttempt + this.#policy.snapshotEveryMs - this.#now());
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      const idle = (globalThis as { requestIdleCallback?: Idle }).requestIdleCallback;
      if (typeof idle === "function") {
        idle(
          () => {
            this.#run();
          },
          { timeout: Math.max(50, this.#policy.snapshotEveryMs) },
        );
      } else {
        this.#run();
      }
    }, wait);
  }

  /** Takes a snapshot now, whatever the schedule says (tests, benchmarks); returns its size, or `null` when none could be taken. */
  takeNow(): number | null {
    this.#dirty = false;
    this.#lastAttempt = this.#now();
    let bytes: Uint8Array;
    try {
      bytes = this.#source.take();
    } catch (error) {
      this.#source.failed(error);
      return null;
    }
    if (bytes.byteLength > this.#policy.maxSnapshotBytes) {
      if (!this.#toldTooLarge) {
        this.#toldTooLarge = true;
        this.#source.tooLarge(bytes.byteLength, this.#policy.maxSnapshotBytes);
      }
      return bytes.byteLength;
    }
    const data = bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength ? (bytes.buffer as ArrayBuffer) : (bytes.slice().buffer as ArrayBuffer);
    this.#last = { data, takenAt: this.#now() };
    return bytes.byteLength;
  }

  /** Stops scheduling (the core is closed). The kept snapshot stays readable. */
  stop(): void {
    this.#stopped = true;
    if (this.#timer !== undefined) clearTimeout(this.#timer);
    this.#timer = undefined;
  }

  /** Pauses scheduling while the core is restarting; {@link SnapshotKeeper.resume} picks up again. */
  pause(): void {
    if (this.#timer !== undefined) clearTimeout(this.#timer);
    this.#timer = undefined;
    this.#scheduled = false;
    this.#paused = true;
  }

  /** Resumes after {@link SnapshotKeeper.pause}; a change seen meanwhile is snapshotted. */
  resume(): void {
    this.#paused = false;
    if (this.#dirty) this.changed();
  }

  #run(): void {
    this.#scheduled = false;
    if (this.#stopped || this.#paused || !this.#dirty) return;
    this.takeNow();
  }
}

/** A copy of `snapshot` whose generation floor (the u64 after the store count, SPEC 5.9; ADR-040 widened it) is at least `floor` (ADR-022). */
export function withGenerationFloor(snapshot: Uint8Array, floor: number): Uint8Array {
  const copy = snapshot.slice();
  if (copy.byteLength < 12) return copy;
  const view = new DataView(copy.buffer, copy.byteOffset, copy.byteLength);
  const wanted = BigInt(Math.max(0, Math.floor(floor)));
  if (view.getBigUint64(4, true) < wanted) view.setBigUint64(4, wanted, true);
  return copy;
}

/** A snapshot with no stores whose only effect is to raise the generation counter of a fresh core to `floor`. */
export function emptySnapshot(schemaHash: bigint, floor: number): Uint8Array {
  return encodeSnapshot({ generationFloor: Math.max(0, Math.floor(floor)), schemaHash, types: [], description: "", stores: [] });
}

/**
 * The handles of the records in a snapshot, or `null` when it does not decode (a layout this runtime does not read): its
 * stores, and the recreation records of its query handles (ADR-059), which the core re-issues on restore as it restores a store.
 */
export function snapshotStoreHandles(snapshot: Uint8Array): Handle[] | null {
  try {
    return decodeSnapshot(snapshot).stores.map((store) => store.handle);
  } catch {
    return null;
  }
}

/** What a transport's `restart` resolves with (ADR-049 decision 3.4). */
export interface RestartResult {
  /** The `Hello` of the new instance. */
  readonly hello: HelloPayload;
  /** How old the restored snapshot was, in ms; `null` when there was none (or it was refused), and the stores were not restored. */
  readonly restoredFromAgeMs: number | null;
  /** The handles of the stores and query handles the restore brought back (ADR-022, ADR-059: the same handles); `null` when they cannot be read off the snapshot. */
  readonly storeHandles: readonly Handle[] | null;
}

/** The failure every call and stream in flight at a trap ends with when the core restarts (ADR-049 decision 3.4.2). */
export function restartedError(trap: Error): UndraTransportError {
  return new UndraTransportError(
    "restarted",
    msg(156, errorMessage(trap)),
    { cause: trap },
  );
}

// ----- the event ------------------------------------------------------------------------------------

/** A frame of a trap's stack as a line: `symbol (0xoffset)`. */
function formatFrame(frame: UndraPanicFrame): string {
  return `${frame.symbol ?? "<unknown>"} (0x${frame.address.toString(16)})`;
}

/** What `LoadOptions.onCoreRestarted` receives (ADR-049 decision 3.4.6). */
export interface CoreRestartInfo {
  /** The panic that trapped the core. */
  readonly report: UndraPanicReport;
  /** How old the snapshot the stores came back from was, in ms: store writes made after it are lost. `null` when there was none, and no store came back. */
  readonly restoredFromAgeMs: number | null;
  /** How many calls and streams in flight were rejected with `UndraTransportError("restarted")`. */
  readonly rejectedCalls: number;
  /** How many objects the app holds went stale: objects that are neither stores nor query handles, and stores and query handles the snapshot did not have (created after it was taken). Calls on them are refused. */
  readonly staleObjects: number;
}

/**
 * A wasm core trapped and was restarted from its last snapshot (ADR-049): what `onError` receives after a
 * restart, the same as `onCoreRestarted`. It is an {@link UndraUnhandledError} (operation `"wasm core"`) whose
 * `error` is the panic, so an `onError` that forwards every value to a crash reporter forwards this one too.
 */
export class UndraCoreRestarted extends UndraUnhandledError implements CoreRestartInfo {
  override readonly name: string = "UndraCoreRestarted";
  /** The panic that trapped the core. */
  readonly report: UndraPanicReport;
  /** How old the restored snapshot was, in ms; `null` when no store came back. */
  readonly restoredFromAgeMs: number | null;
  /** Calls and streams in flight that failed with `UndraTransportError("restarted")`. */
  readonly rejectedCalls: number;
  /** Objects the app holds that went stale. */
  readonly staleObjects: number;

  /** @param info What happened. @param trap The trap. */
  constructor(info: CoreRestartInfo, trap: Error) {
    super("wasm core", new UndraCallError.Panicked(info.report.message, info.report.frames.map(formatFrame).join("\n"), { cause: trap }), trap);
    this.message = info.restoredFromAgeMs === null ? msg(157, info.report.message) : msg(243, info.report.message, info.restoredFromAgeMs);
    this.report = info.report;
    this.restoredFromAgeMs = info.restoredFromAgeMs;
    this.rejectedCalls = info.rejectedCalls;
    this.staleObjects = info.staleObjects;
  }
}

// ----- the restart sequence ----------------------------------------------------------------------------

/**
 * What a restart brings back: a core, not started, that is started and given a snapshot. A `WasmMainTransport` is one (the
 * worker's core is); `Recovering` makes one of the in-process host (`WasmHost`), whose snapshot operations are functions
 * (`wasm-snapshot.ts`).
 */
export interface Restartable {
  /** Starts the new instance with `handler`. */
  start(handler: TransportHandler): Promise<HelloPayload>;
  /** `undra_restore`; rejects `UndraRestoreError` for a refused snapshot. */
  restore(bytes: Uint8Array): Promise<void>;
}

/** `run`'s result as a promise, or its failure as a rejection. */
function promised<T>(run: () => T): Promise<T> {
  try {
    return Promise.resolve(run());
  } catch (error) {
    return Promise.reject(error);
  }
}

/**
 * A snapshot keeper over a core on this thread: `take` is its `takeSnapshot`, and a too-large snapshot or a failure is
 * said through `log` (the worker of `wasm-worker` makes one too).
 */
export function keeperOf(
  policy: SnapshotPolicy,
  take: () => Uint8Array,
  log: (level: number, target: string, message: string) => void,
): SnapshotKeeper {
  return new SnapshotKeeper(policy, {
    take,
    tooLarge: (bytes, limit) => {
      log(3, "undra::recovery", msg(158, bytes, limit));
    },
    failed: (error) => {
      log(4, "undra::recovery", msg(159, errorMessage(error)));
    },
  });
}

/**
 * The restart of a core on this thread (ADR-049 decision 3.4.3): `twin` (a twin of the transport that trapped, which the
 * caller has made its transport already, so that the new instance's own port calls are answered) is started with
 * `handler`, and the last snapshot `keeper` kept restored with its generation floor raised to at least `floor`, so that no
 * handle the host still holds can be issued again to another object (ADR-022). Without a snapshot (or when the core
 * refuses it), an empty one raises the floor alone and no store comes back.
 */
export async function restartHere(
  twin: Restartable,
  handler: TransportHandler,
  keeper: SnapshotKeeper,
  floor: number,
  log: (level: number, target: string, message: string) => void,
): Promise<RestartResult> {
  keeper.pause();
  try {
    const hello = await twin.start(handler);
    const kept = keeper.last;
    if (kept !== null) {
      const bytes = withGenerationFloor(new Uint8Array(kept.data), floor);
      try {
        await twin.restore(bytes);
        return { hello, restoredFromAgeMs: Math.max(0, Date.now() - kept.takenAt), storeHandles: snapshotStoreHandles(bytes) };
      } catch (error) {
        if (!(error instanceof UndraRestoreError)) throw error;
        log(4, "undra::recovery", msg(160, error.code));
      }
    }
    try {
      await twin.restore(emptySnapshot(hello.schemaHash, floor));
    } catch (error) {
      if (!(error instanceof UndraRestoreError)) throw error;
    }
    return { hello, restoredFromAgeMs: null, storeHandles: [] };
  } finally {
    keeper.resume();
  }
}

/**
 * What a core gives its crash recovery: the state and the few operations the restart sequence needs that `UndraCore`
 * does not offer publicly.
 *
 * @internal Made by `UndraCore` for {@link CrashRecovery}.
 */
export interface RecoveryHost {
  /** The core. */
  readonly core: UndraCore;
  /** The handles constructed through the core. */
  readonly handles: Set<Handle>;
  /** The signals observed, per handle. */
  readonly observed: Map<Handle, Set<number>>;
  /** Fails every call, stream and `observe` in flight with `error`; returns how many calls and streams. */
  fail(error: Error): number;
  /** Ends the core: the channel is lost for good (`onClose`). */
  lose(error: Error): void;
  /** Hands `error` to `onError`, guarded as the core's own reports are. */
  deliver(error: UndraUnhandledError): void;
  /** Builds the panic report of `trap` and hands it to `onPanic`; the host has what builds it (`CrashRecovery.panics`) by now. */
  panicked(trap: Error): UndraPanicReport;
  /**
   * The registered ports. Each one releases at a restart what the instance that trapped held through it
   * (`PortImpl.dispose`: its WebSocket connections, its databases and their open transactions), which nobody would
   * close or roll back otherwise; the ports then serve the new instance.
   */
  readonly ports: ReadonlyMap<number, PortImpl>;
}

/**
 * Crash recovery for a web core (ADR-049 decision 3), what `LoadOptions.recovery` takes: make one with
 * {@link crashRecovery}, one per core.
 */
export interface CrashRecovery {
  /** The settings, defaults applied. */
  readonly options: ResolvedRecovery;
  /** The last snapshot kept on this thread (`wasm-main`; in `wasm-worker` mode the worker keeps it), or `null`. For devtools, tests and benchmarks. */
  readonly lastSnapshot: KeptSnapshot | null;
  /**
   * What builds the panic report of a trap (`UndraCoreRestarted.report` carries one): a core with recovery has it from the
   * start, where a core that only has `onPanic` loads it when it starts.
   *
   * @internal Read by `UndraCore`.
   */
  readonly panics: PanicSupport;
  /** Takes and keeps a snapshot now (`wasm-main`), whatever the schedule says: returns its size, or `null` when none could be taken. For tests and benchmarks. */
  keepSnapshotNow(): number | null;
  /**
   * Connects the recovery to the core that loads with it: returns the transport the core then uses, `transport` behind
   * the layer that runs the restart sequence (it sees every trap, every call and every reply first).
   *
   * @internal Called by `UndraCore`; throws when this recovery already belongs to a core.
   */
  attach(transport: CoreTransport, host: RecoveryHost, onCoreRestarted?: (event: UndraCoreRestarted) => void): CoreTransport;
}

/**
 * Crash recovery for a web core, for `LoadOptions.recovery` (ADR-049 decision 3; off unless given):
 *
 * ```ts
 * await UndraCore.load({ mode: "wasm-main", wasm, expectedSchemaHash: UndraIds.schemaHash, recovery: crashRecovery() });
 * ```
 *
 * The runtime keeps a snapshot of the stores, at most once per `snapshotEveryMs` while they change (none larger than
 * `maxSnapshotBytes`, in the worker in `wasm-worker` mode). When the core traps: `onPanic` hears the panic; every call and
 * stream in flight fails with `UndraTransportError("restarted")` (generated calls: `UndraCallError.Unavailable`; it may or
 * may not have run, and it is not retried); the registered ports release what the instance held through them
 * (`PortImpl.dispose`: WebSocket connections and event streams close, open transactions roll back, databases close);
 * the same compiled module is instantiated again and the snapshot restored
 * (stores and query handles keep their handles, ADR-059); every observed store and query handle is observed again; then
 * `onCoreRestarted` and `onError` hear an {@link UndraCoreRestarted}. Lost: store writes after the last snapshot, stores
 * and query handles created after it, objects that are neither, the core's running tasks and timers, and whatever the
 * core held outside its stores (re-apply it in `onCoreRestarted`). Past `maxRestarts` within `perMs` the core stays dead
 * and `onClose` reports the trap. Wasm modes only (a native core contains its panics). A core loaded without it ships none
 * of this code.
 */
export function crashRecovery(options: RecoveryOptions = {}): CrashRecovery {
  const settings = resolveRecovery(options);
  let attached: Recovering | null = null;
  return {
    options: settings,
    panics: panicSupport,
    get lastSnapshot() {
      return attached?.keeper?.last ?? null;
    },
    keepSnapshotNow: () => attached?.keeper?.takeNow() ?? null,
    attach(transport, host, onCoreRestarted) {
      if (attached !== null) throw new UndraTransportError("unsupported", msg(161));
      attached = new Recovering(transport, settings, host, onCoreRestarted);
      return attached;
    },
  };
}

/** What a call made while the core restarts fails with (ADR-049 decision 3.4.2). */
function restarting(): UndraTransportError {
  return new UndraTransportError("restarted", msg(162));
}

/**
 * One core's crash recovery: the transport the core uses when it loads with `recovery`. It passes everything through to
 * the core's own transport, and it is where the restart happens: it sees a trap before the core does (and restarts
 * instead of letting the core close), refuses calls while the core restarts, holds back releases until it is back, drops
 * the port replies that belong to the instance that trapped, and keeps the snapshots (`wasm-main`).
 */
class Recovering implements CoreTransport {
  /** The core's own transport; in `wasm-main`, a restart replaces it with its twin. */
  #inner: CoreTransport;
  readonly #settings: ResolvedRecovery;
  readonly #host: RecoveryHost;
  /** `#inner` when the core runs on this thread (`wasm-main`): snapshots are kept here, and the restart runs here. */
  #here: WasmHost | null;
  /** The snapshots of a core on this thread (`null` in `wasm-worker`: the worker keeps them). */
  readonly keeper: SnapshotKeeper | null;
  readonly #onRestarted: ((event: UndraCoreRestarted) => void) | undefined;
  /** The core's handler, and what the transport is given instead (see `start`). */
  #handler: TransportHandler | null = null;
  #wrapped: TransportHandler | null = null;
  /** The releases sent while the core restarted, per handle (a give-back or a wrapper's own): sent once it is back. */
  readonly #releasedWhileDown = new Map<Handle, number>();
  /** When the restarts within the budget's window happened (`Date.now`). */
  #times: number[] = [];
  /** Counts recoveries: a re-attach that a newer trap overtook stops. */
  #run = 0;
  /** While the trapped core is being instantiated again: calls fail with "restarted". */
  #restarting = false;
  /** The highest floor a restart used: handles that went stale are forgotten, but the app's wrappers keep them (ADR-022). */
  #floorUsed = 0;
  /** A trap the transport reported while a restart was under way (the new instance trapped before the restart answered). */
  #trappedWhileRestarting: UndraTransportError | null = null;
  /** Bumped by every restart: a port reply that settles later belongs to the epoch of its call. */
  #epoch = 0;
  declare readonly callSync?: (payload: Uint8Array) => Uint8Array;
  declare readonly callSyncParts?: (head: Uint8Array, tail: Uint8Array) => Uint8Array;
  declare readonly stats?: () => Promise<string | null>;
  declare readonly snapshot?: () => Promise<Uint8Array>;
  declare readonly restore?: (bytes: Uint8Array) => Promise<void>;
  declare readonly portAdded?: (portId: number, impl: PortImpl) => void;

  constructor(inner: CoreTransport, settings: ResolvedRecovery, host: RecoveryHost, onRestarted: ((event: UndraCoreRestarted) => void) | undefined) {
    this.#inner = inner;
    this.#settings = settings;
    this.#host = host;
    this.#here = inner instanceof WasmHost ? inner : null;
    this.keeper =
      this.#here === null
        ? null
        : keeperOf(
            settings,
            () => takeSnapshot(this.#here as WasmHost),
            (level, target, message) => {
              this.#log(level, target, message);
            },
          );
    this.#onRestarted = onRestarted;
    // The optional members exactly as the transport has them (the core tells the modes apart by them), called on whichever
    // transport is current: a twin has the same ones.
    const self = this as { -readonly [K in keyof Recovering]?: Recovering[K] };
    const current = (): Required<CoreTransport> => this.#inner as Required<CoreTransport>;
    if (inner.callSync !== undefined) {
      self.callSync = (payload) => {
        if (this.#restarting) throw restarting();
        return this.#guard(() => current().callSync(payload));
      };
    }
    if (inner.callSyncParts !== undefined) {
      self.callSyncParts = (head, tail) => {
        if (this.#restarting) throw restarting();
        return this.#guard(() => current().callSyncParts(head, tail));
      };
    }
    if (inner.stats !== undefined) self.stats = () => (this.#restarting ? Promise.resolve(null) : current().stats());
    if (this.#here !== null) {
      // The in-process host: its snapshot operations are functions over whichever host is current (a restart replaces it with its twin).
      self.snapshot = () => (this.#restarting ? Promise.reject(restarting()) : promised(() => takeSnapshot(this.#here as WasmHost)));
      self.restore = (bytes) => (this.#restarting ? Promise.reject(restarting()) : promised(() => restoreInto(this.#here as WasmHost, bytes)));
    } else {
      if (inner.snapshot !== undefined) self.snapshot = () => (this.#restarting ? Promise.reject(restarting()) : current().snapshot());
      if (inner.restore !== undefined) self.restore = (bytes) => (this.#restarting ? Promise.reject(restarting()) : current().restore(bytes));
    }
    if (inner.portAdded !== undefined) {
      self.portAdded = (portId, impl) => {
        current().portAdded(portId, impl);
      };
    }
  }

  get mode(): string {
    return this.#inner.mode;
  }

  get synchronous(): boolean {
    return this.#inner.synchronous;
  }

  start(handler: TransportHandler): Promise<HelloPayload> {
    const epoch = (): number => this.#epoch;
    this.#handler = handler;
    this.#wrapped = {
      ...handler,
      changeSet: (payload) => {
        handler.changeSet(payload);
        // A store changed: a snapshot is due (the worker keeps its own in `wasm-worker` mode).
        this.keeper?.changed();
      },
      portCall: (call) => {
        // A reply that settles after a restart is for a call of the instance that trapped: it must never reach the new one.
        // Its id reads 0 by then (no call the core waits for has id 0), and `send` drops it.
        const of = epoch();
        const { portId, methodId, portCallId, args } = call;
        return handler.portCall({
          portId,
          methodId,
          args,
          get portCallId() {
            return of === epoch() ? portCallId : 0;
          },
        });
      },
      closed: (error) => {
        // A failure while a restart is under way is the restart's to handle: when the restart fails it says so itself;
        // when it answers anyway (the new instance trapped after its restore, before the restart answered), the trap is
        // kept here and the restart loop takes it as the next trap instead of a dead core passing for a restarted one.
        if (this.#restarting) {
          if (isTrap(error)) this.#trappedWhileRestarting = error;
          return;
        }
        if (isTrap(error) && this.#mayRestart()) {
          void this.#recover(error, this.#host.panicked(error));
          return;
        }
        handler.closed(error);
      },
    };
    return this.#inner.start(this.#wrapped);
  }

  // The control messages (ADR-057), with the rules the framed `send` had: while the core restarts nothing reaches it except a
  // port reply of the new instance and a release (kept, and sent once the core is back); a port reply of the instance that
  // trapped is dropped.

  sendCall(head: Uint8Array, tail?: Uint8Array): void {
    this.#reaching(() => this.#inner.sendCall(head, tail));
  }

  observe(handle: Handle, signalId: number, on: boolean): void {
    this.#reaching(() => this.#inner.observe(handle, signalId, on));
  }

  cancel(callId: number): void {
    this.#reaching(() => this.#inner.cancel(callId));
  }

  streamCredit(callId: number, credit: number): void {
    this.#reaching(() => this.#inner.streamCredit(callId, credit));
  }

  event(portId: number, methodId: number, payload: Uint8Array): void {
    this.#reaching(() => this.#inner.event(portId, methodId, payload));
  }

  timerFired(timerId: number): void {
    this.#reaching(() => this.#inner.timerFired(timerId));
  }

  release(handle: Handle): void {
    if (this.#restarting) {
      // The core keeps the object meanwhile; it is released once the core is back.
      this.#releasedWhileDown.set(handle, (this.#releasedWhileDown.get(handle) ?? 0) + 1);
      return;
    }
    this.#guard(() => {
      this.#inner.release(handle);
    });
  }

  portReply(reply: Uint8Array): void {
    // The new instance's own port calls are answered during the restart too; a stale reply (id 0, see `start`) never is.
    if (reply.byteLength >= 4 && new DataView(reply.buffer, reply.byteOffset, 4).getUint32(0, true) === 0) return;
    this.#guard(() => {
      this.#inner.portReply(reply);
    });
  }

  /** Runs a message to the core, which refuses with "restarted" while the core restarts. */
  #reaching(send: () => void): void {
    if (this.#restarting) throw restarting();
    this.#guard(send);
  }

  close(): void {
    this.keeper?.stop();
    this.#inner.close();
  }

  /** Runs `run`; a trap it throws is "restarted" when the core will recover from it, which the caller then sees. */
  #guard<T>(run: () => T): T {
    try {
      return run();
    } catch (error) {
      throw isTrap(error) && (this.#restarting || this.#mayRestart()) ? restartedError(error) : error;
    }
  }

  /** Whether a trap now would be recovered from: the transport can restart, and the budget allows one more. */
  #mayRestart(): boolean {
    if ((this.#here === null && typeof this.#inner.restart !== "function") || this.#host.core.closed) return false;
    const since = Date.now() - this.#settings.perMs;
    this.#times = this.#times.filter((at) => at > since);
    return this.#times.length < this.#settings.maxRestarts;
  }

  /**
   * The highest handle generation the host holds, and never below the floor of an earlier restart (whose stale handles the
   * host forgot while wrappers may keep them): the floor a restore must not go below (ADR-022).
   */
  #floor(): number {
    const host = this.#host;
    let floor = this.#floorUsed;
    for (const set of [host.handles, host.observed.keys(), this.#releasedWhileDown.keys()]) {
      for (const handle of set) floor = Math.max(floor, handleGeneration(handle));
    }
    this.#floorUsed = floor;
    return floor;
  }

  /**
   * Brings the core back: on this thread, a twin of the transport that trapped becomes the core's transport (first, so
   * that the new instance's own port calls are answered) and is started; in `wasm-worker`, the worker restarts it.
   */
  #restart(floor: number): Promise<RestartResult> {
    const here = this.#here;
    if (here === null || this.keeper === null || this.#wrapped === null) {
      return (this.#inner as Transport & { restart(floor: number): Promise<RestartResult> }).restart(floor);
    }
    const next = twin(here);
    here.close();
    this.#inner = next;
    this.#here = next;
    const restartable: Restartable = { start: (handler) => next.start(handler), restore: (bytes) => promised(() => restoreInto(next, bytes)) };
    return restartHere(restartable, this.#wrapped, this.keeper, floor, (level, target, message) => {
      this.#log(level, target, message);
    });
  }

  /** The core's log (through its handler: a record of this layer is never a panic's or a dev server's). */
  #log(level: number, target: string, message: string): void {
    this.#handler?.log(level, target, message);
  }

  /** Starts or ends the restart window: calls are refused meanwhile, and starting one begins a new epoch of port replies. */
  #setRestarting(on: boolean): void {
    this.#restarting = on;
    if (on) this.#epoch++;
  }

  /**
   * The restart sequence of ADR-049 decision 3.4: the panic report went to `onPanic` already; every call and stream in
   * flight fails with "restarted"; the core comes back from the last snapshot; the stores and the query handles (which the
   * core re-issues on their own handles, ADR-059) are observed again; then `onCoreRestarted` and `onError`. A trap during
   * it counts against the budget like any other.
   */
  async #recover(firstTrap: UndraTransportError, firstReport: UndraPanicReport): Promise<void> {
    const host = this.#host;
    const run = ++this.#run;
    let trap = firstTrap;
    let report = firstReport;
    this.#setRestarting(true);
    const rejectedCalls = host.fail(restartedError(trap));
    let result: RestartResult;
    for (;;) {
      this.#times.push(Date.now());
      this.#trappedWhileRestarting = null;
      // What the instance that trapped held through the ports (connections, a transaction) goes with it.
      for (const impl of new Set(host.ports.values())) {
        try {
          impl.dispose?.();
        } catch (error) {
          host.core.report(error, "port dispose");
        }
      }
      try {
        result = await this.#restart(this.#floor());
        const late = this.#trappedWhileRestarting as UndraTransportError | null;
        this.#trappedWhileRestarting = null;
        if (late !== null) throw late;
        break;
      } catch (error) {
        if (host.core.closed || run !== this.#run) return;
        if (isTrap(error)) {
          trap = error;
          report = host.panicked(error);
          if (this.#mayRestart()) continue;
        }
        this.#setRestarting(false);
        this.#log(4, "undra::recovery", msg(163, errorMessage(error)));
        host.lose(isTrap(error) ? error : trap);
        return;
      }
    }
    if (host.core.closed || run !== this.#run) return;
    host.core.hello = result.hello;
    this.#setRestarting(false);
    let staleObjects: number | null;
    try {
      staleObjects = await this.#reattach(result, run);
    } catch (error) {
      // A trap while observing again: the transport reported it, and that report starts the next round.
      if (!overtaken(error) && !host.core.closed) host.core.report(error, "core restart");
      return;
    }
    if (staleObjects === null || host.core.closed || run !== this.#run) return;
    const event = new UndraCoreRestarted({ report, restoredFromAgeMs: result.restoredFromAgeMs, rejectedCalls, staleObjects }, trap);
    this.#log(3, "undra::recovery", msg(164, event.message, rejectedCalls, staleObjects));
    try {
      this.#onRestarted?.(event);
    } catch (thrown) {
      this.#log(4, "undra::runtime", msg(165, errorMessage(thrown)));
    }
    host.deliver(event);
  }

  /**
   * After a restart: what was released meanwhile is released, and the stores and query handles the snapshot brought back
   * are observed again (their values reach the mirror; the core builds a query handle again when it is first observed,
   * ADR-059, so the wrapper keeps its handle). Returns how many objects went stale (not in the snapshot: objects that are
   * neither, and stores and query handles created after it), or `null` when a newer trap overtook this round.
   */
  async #reattach(result: RestartResult, run: number): Promise<number | null> {
    const host = this.#host;
    const core = host.core;
    const restored = result.storeHandles === null ? null : new Set(result.storeHandles);
    let stale = 0;
    for (const handle of new Set([...host.handles, ...host.observed.keys()])) {
      if (restored === null || restored.has(handle)) continue;
      stale++;
      host.handles.delete(handle);
      host.observed.delete(handle);
    }
    // What was released meanwhile goes to the restored core as the references it was: each one a bare release (what
    // `core.release` did locally, unregistering and forgetting, ran when the wrapper closed). A handle an open wrapper
    // holds now is one whose queued releases were give-backs of extra references (a superseded wrapper's finalizer, a
    // reply that carried the handle again): the restored core counts what the snapshot held, which may not include
    // them, and sending them could release the live wrapper's only reference, so they are dropped (a leaked reference
    // until the core closes, at worst).
    for (const [handle, count] of this.#releasedWhileDown) {
      const live = wrapperOf(core, handle);
      if (live !== undefined && !live.closed) continue;
      for (let i = 0; i < count; i++) core._giveBack(handle);
    }
    this.#releasedWhileDown.clear();
    core._era++;
    const observing: Array<Promise<void>> = [];
    for (const [handle, signals] of [...host.observed]) {
      for (const signalId of signals) observing.push(core.observe(handle, signalId, true));
    }
    const settled = await Promise.allSettled(observing);
    if (run !== this.#run || core.closed) return null;
    const trapped = settled.find((s): s is PromiseRejectedResult => s.status === "rejected" && overtaken(s.reason));
    if (trapped !== undefined) throw trapped.reason;
    return stale;
  }
}

/** Whether `error` means that a newer trap overtook a restart in progress: the trap itself, or what it failed the waiting work with. */
function overtaken(error: unknown): boolean {
  return isTrap(error) || (error instanceof UndraTransportError && error.reason === "restarted");
}
