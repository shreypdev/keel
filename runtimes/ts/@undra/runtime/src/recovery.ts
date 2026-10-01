import { UndraCallError, UndraUnhandledError } from "./call-error.js";
import { UndraTransportError } from "./errors.js";
import { errorMessage } from "./platform.js";
import { type Handle, type HelloPayload, decodeSnapshot, encodeSnapshot } from "./wire/index.js";

/*
 * Recovering a web core that trapped (ADR-049 decision 3): the options of `LoadOptions.recovery`, the snapshot
 * keeper that runs where the core runs (the main thread in `wasm-main`, the worker in `wasm-worker`), the panic
 * report built from the core's FATAL record and the trap's stack (ADR-046 decision 4.4, the minimal part), and
 * the event an app hears after a restart.
 */

/** `LoadOptions.recovery` as an object: every field is optional. */
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
  readonly snapshotEveryMs: number;
  readonly maxSnapshotBytes: number;
  readonly maxRestarts: number;
  readonly perMs: number;
}

/** The defaults of {@link RecoveryOptions} (ADR-049). */
export const DEFAULT_RECOVERY: ResolvedRecovery = Object.freeze({
  snapshotEveryMs: 1000,
  maxSnapshotBytes: 4 * 1024 * 1024,
  maxRestarts: 3,
  perMs: 60_000,
});

/** `true`, an object or nothing, as the settings a core runs with; `null` when recovery is off (the default). */
export function resolveRecovery(option: boolean | RecoveryOptions | undefined): ResolvedRecovery | null {
  if (option === undefined || option === false) return null;
  const given = option === true ? {} : option;
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
  readonly snapshotEveryMs: number;
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

/** A copy of `snapshot` whose generation floor (the u32 after the store count, SPEC 5.9) is at least `floor` (ADR-022). */
export function withGenerationFloor(snapshot: Uint8Array, floor: number): Uint8Array {
  const copy = snapshot.slice();
  if (copy.byteLength < 8) return copy;
  const view = new DataView(copy.buffer, copy.byteOffset, copy.byteLength);
  if (view.getUint32(4, true) < floor) view.setUint32(4, floor >>> 0, true);
  return copy;
}

/** A snapshot with no stores whose only effect is to raise the generation counter of a fresh core to `floor`. */
export function emptySnapshot(schemaHash: bigint, floor: number): Uint8Array {
  return encodeSnapshot({ generationFloor: floor >>> 0, schemaHash, types: [], description: "", stores: [] });
}

/** The handles of the stores in a snapshot, or `null` when it does not decode (a layout this runtime does not read). */
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
  /** The handles of the stores the restore brought back (ADR-022: the same handles); `null` when they cannot be read off the snapshot. */
  readonly storeHandles: readonly Handle[] | null;
}

// ----- the panic report (ADR-046 decision 4.4, minimal) ----------------------------------------------

/**
 * What the TypeScript runtime knows about a panic that trapped a wasm core (ADR-046 decision 4.4, the part this
 * runtime implements): the panic's own record, which the core logs at FATAL level with target `undra::panic`
 * before it traps, and the stack of the trap. Delivered to `LoadOptions.onPanic` once per trap, before a
 * restart (ADR-049) and whether or not recovery is on.
 */
export interface UndraPanicReport {
  /** The panic message from the core's FATAL `undra::panic` record; the trap's own text when the core logged none (a stack overflow, out of memory). */
  readonly message: string;
  /** `file:line[:column]` of the panic when the record carried it (a panic before `undra_init`), else `""`. */
  readonly location: string;
  /** What the runtime was running when the core trapped, as far as it knows: `"wasm-main"` or `"wasm-worker"`, and the trap's text. */
  readonly operation: string;
  /** The frames of the trap's stack that are in the wasm module (`wasm-function[123]:0x4567`, or a name in a build with names), innermost first. */
  readonly frames: readonly string[];
  /** The schema hash of the core. */
  readonly schemaHash: bigint;
  /** The text of the trap (`RuntimeError: unreachable`, ...). */
  readonly trap: string;
}

/** The stack of a trap: the engine's, carried as the `cause` of the transport's error (or its own stack). */
export function trapStack(error: unknown): string {
  const cause = (error as { cause?: unknown } | null)?.cause;
  const stack = (cause as { stack?: unknown } | null | undefined)?.stack ?? (error as { stack?: unknown } | null)?.stack;
  return typeof stack === "string" ? stack : "";
}

/** The panic report of a trap, from the core's last FATAL `undra::panic` record (if any) and the trap. */
export function panicReport(record: string | null, trap: Error, schemaHash: bigint, mode: string): UndraPanicReport {
  const cause = (trap as { cause?: unknown }).cause;
  const trapText = cause instanceof Error ? `${cause.name}: ${cause.message}` : trap.message;
  let message = record ?? trapText;
  let location = "";
  // The record of a panic before `undra_init` ends with " at file:line".
  const at = record === null ? null : /^(.*) at ([^\s]+:\d+(?::\d+)?)$/s.exec(record);
  if (at !== null) {
    message = at[1] as string;
    location = at[2] as string;
  }
  // A runtime record is `"<what>: <message>\n<backtrace>"` on native; on wasm it is the message alone.
  const frames = trapStack(trap)
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.includes("wasm-function[") || line.includes(".wasm"))
    .map((line) => line.replace(/^at\s+/, ""));
  return { message, location, operation: `${mode}: ${trapText}`, frames, schemaHash, trap: trapText };
}

/** Whether `error` is the trap of a wasm core. */
export function isTrap(error: unknown): error is UndraTransportError {
  return error instanceof UndraTransportError && error.reason === "trap";
}

/** The failure every call and stream in flight at a trap ends with when the core restarts (ADR-049 decision 3.4.2). */
export function restartedError(trap: Error): UndraTransportError {
  return new UndraTransportError(
    "restarted",
    `the wasm core trapped and was restarted from its last snapshot; this call may or may not have run before the trap, and it is not retried (${errorMessage(trap)})`,
    { cause: trap },
  );
}

// ----- the event ------------------------------------------------------------------------------------

/** What `LoadOptions.onCoreRestarted` receives (ADR-049 decision 3.4.6). */
export interface CoreRestartInfo {
  /** The panic that trapped the core. */
  readonly report: UndraPanicReport;
  /** How old the snapshot the stores came back from was, in ms: store writes made after it are lost. `null` when there was none, and no store came back. */
  readonly restoredFromAgeMs: number | null;
  /** How many calls and streams in flight were rejected with `UndraTransportError("restarted")`. */
  readonly rejectedCalls: number;
  /** How many objects the app holds went stale: objects that are not stores (query handles excepted: they are re-created), and stores the snapshot did not have. Calls on them are refused. */
  readonly staleObjects: number;
}

/**
 * A wasm core trapped and was restarted from its last snapshot (ADR-049): what `onError` receives after a
 * restart, the same as `onCoreRestarted`. It is an {@link UndraUnhandledError} (operation `"core restart"`) whose
 * `error` is the panic, so an `onError` that forwards every value to a crash reporter forwards this one too.
 */
export class UndraCoreRestarted extends UndraUnhandledError implements CoreRestartInfo {
  override readonly name: string = "UndraCoreRestarted";
  readonly report: UndraPanicReport;
  readonly restoredFromAgeMs: number | null;
  readonly rejectedCalls: number;
  readonly staleObjects: number;

  /** @param info What happened. @param trap The trap. */
  constructor(info: CoreRestartInfo, trap: Error) {
    super("core restart", new UndraCallError.Panicked(info.report.message, info.report.frames.join("\n"), { cause: trap }), trap);
    this.report = info.report;
    this.restoredFromAgeMs = info.restoredFromAgeMs;
    this.rejectedCalls = info.rejectedCalls;
    this.staleObjects = info.staleObjects;
  }
}
