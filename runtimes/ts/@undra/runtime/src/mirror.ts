import { UndraError } from "./errors.js";
import { batch } from "./signal.js";
import { ALL_SIGNALS, type ChangeEntry, ChangeOp, type Handle, decodeChangeSet } from "./wire/index.js";

/** Applies one signal update to the store registered for a handle (`UndraStore._apply`). */
export type ApplyFn = (signalId: number, op: ChangeOp, value: Uint8Array) => void;

/** How a store registers with the {@link Mirror}. */
export interface RegisterOptions {
  /**
   * The store's signals declared `#[undra(no_coalesce)]`: the mirror applies every entry of
   * these, in order, and announces each one on its own instead of folding them into one per
   * drain. Generated stores pass them; nothing else needs to.
   */
  readonly noCoalesce?: Iterable<number>;
}

/** What one drain did; see {@link Mirror.addDrainListener}. */
export interface DrainStats {
  /** Change-sets the drain consumed (received since the previous drain). */
  readonly changeSets: number;
  /** Entries those change-sets carried. */
  readonly entries: number;
  /** Entries applied to stores after merging: calls of their apply functions. */
  readonly appliedEntries: number;
  /** How long the drain took, in milliseconds (`performance.now()`, so as coarse as the platform makes it). */
  readonly durationMs: number;
}

/** Called after each drain; see {@link Mirror.addDrainListener}. */
export type DrainListener = (stats: DrainStats) => void;

/** The counters of a {@link Mirror} (docs/SPEC.md section 17.1), also reported by `UndraCore.stats()`. */
export interface MirrorStats {
  /** Change-sets accepted (malformed ones are not counted). */
  readonly changeSetsReceived: number;
  /** Entries those change-sets carried. */
  readonly entriesReceived: number;
  /** Entries applied to stores after merging. */
  readonly entriesApplied: number;
  /** Drains run (flushes that found something queued). */
  readonly drains: number;
  /** Times the backlog passed its bound and was folded in place. */
  readonly compactions: number;
  /** Signals re-observed because the backlog dropped their merged patch. */
  readonly resyncs: number;
  /** Entries waiting for the next drain. */
  readonly pendingEntries: number;
  /** Bytes those entries hold (their values plus 17 bytes each). */
  readonly pendingBytes: number;
  /** Entries dropped because no store was registered for their handle. */
  readonly droppedEntries: number;
}

/** Options of a {@link Mirror}. */
export interface MirrorOptions {
  /** Receives every failure the mirror cannot report to a caller: a malformed change-set, an `apply` function that throws. Default: rethrow from a microtask. */
  readonly onError?: (error: unknown) => void;
  /**
   * Schedules the drain of change-sets the core produced on its own. Default
   * {@link scheduleFrame}: the next animation frame while a document is visible, a zero-delay
   * task while it is hidden, a microtask where there is no document (Node, workers). Replies,
   * `callSync` and `observe` drain without waiting for it.
   */
  readonly schedule?: (fn: () => void) => void;
  /**
   * Asks the core for the current value of a signal (`observe(handle, signalId, on)` without a
   * waiter): what the mirror does for a signal whose merged patch the backlog dropped. Without
   * it such a signal waits for the core's next full value.
   */
  readonly resync?: (handle: Handle, signalId: number) => void;
  /** When more entries than this wait, the backlog is folded in place. Default 65,536. */
  readonly maxPendingEntries?: number;
  /** When the waiting entries hold more bytes than this, the backlog is folded in place. Default 16 MiB. */
  readonly maxPendingBytes?: number;
}

/** Default of {@link MirrorOptions.maxPendingEntries}. */
export const DEFAULT_MAX_PENDING_ENTRIES = 65_536;
/** Default of {@link MirrorOptions.maxPendingBytes}. */
export const DEFAULT_MAX_PENDING_BYTES = 16 * 1024 * 1024;
/**
 * A merged patch with more operations than this, **or** more op bytes than {@link MAX_MERGED_PATCH_BYTES},
 * is dropped by a compaction and the signal re-observed: the bytes bound what the backlog holds per
 * signal, the operations what a drain replays for it (the core's own op log stops at 4,096 too).
 */
export const MAX_MERGED_PATCH_OPS = 4096;
/** See {@link MAX_MERGED_PATCH_OPS}. */
export const MAX_MERGED_PATCH_BYTES = 1024 * 1024;

/**
 * Rounds one flush runs before it hands the rest to a later flush: a subscriber whose core call
 * changes a signal it is subscribed to would otherwise keep one flush going forever. The same
 * bound as the core's commit (docs/SPEC.md section 16.1).
 */
const MAX_ROUNDS = 1000;

/** What a queued entry costs besides its value: the wire's fixed part (handle, signal id, op, length). */
const ENTRY_OVERHEAD = 17;

/**
 * A rAF still pending after this long means no frame is coming (the document was hidden after the
 * request, an off-screen iframe): the drain runs from a timer instead.
 */
const FRAME_BACKSTOP_MS = 100;

interface FrameGlobals {
  readonly document?: { readonly visibilityState?: string };
  readonly requestAnimationFrame?: (fn: (time: number) => void) => number;
  readonly cancelAnimationFrame?: (id: number) => void;
}

/**
 * The default schedule of a {@link Mirror}: runs `fn` once, at the next animation frame while a
 * document is visible (with a 100 ms timer as a backstop for a frame that never comes), in a
 * zero-delay task while it is not (browsers run no frames for a hidden document), and in a
 * microtask where there is no document (Node, workers), so a drain follows each macrotask there.
 */
export function scheduleFrame(fn: () => void): void {
  const g = globalThis as FrameGlobals;
  const doc = g.document;
  if (doc === undefined) {
    queueMicrotask(fn);
    return;
  }
  const visible = doc.visibilityState === undefined || doc.visibilityState === "visible";
  if (!visible || typeof g.requestAnimationFrame !== "function") {
    setTimeout(fn, 0);
    return;
  }
  let done = false;
  let frame = 0;
  const run = (): void => {
    if (done) return;
    done = true;
    clearTimeout(timer);
    g.cancelAnimationFrame?.(frame);
    fn();
  };
  const timer = setTimeout(run, FRAME_BACKSTOP_MS);
  frame = g.requestAnimationFrame(run);
}

/** The global `performance.now()`, or `Date.now()` where there is none. */
function now(): number {
  return typeof performance === "object" ? performance.now() : Date.now();
}

interface Registration {
  readonly apply: ApplyFn;
  readonly noCoalesce: ReadonlySet<number> | null;
}

interface Waiter {
  readonly signalId: number;
  readonly resolve: () => void;
  readonly reject: (error: unknown) => void;
  timer: ReturnType<typeof setTimeout> | undefined;
}

/** `true` once the signal's merged patch was dropped and nothing re-observed it yet; `false` once re-observed. */
type Awaiting = Map<Handle, Map<number, boolean>>;

/** One signal of one store as a drain or a compaction folds it (docs/SPEC.md section 11). */
class Slot {
  /** The last full value or lazy invalidation; everything that arrived before it is superseded. */
  full: ChangeEntry | null = null;
  /** The keyed patches that arrived after `full`, whole (count and ops), in arrival order. */
  patches: Uint8Array[] = [];
  /** Sum of the patches' op counts. */
  ops = 0;
  /** Sum of the patches' op bytes, counts excluded. */
  opBytes = 0;
  /** Entries folded into this slot. */
  entries = 0;

  constructor(
    readonly handle: Handle,
    readonly signalId: number,
  ) {}

  setFull(entry: ChangeEntry): void {
    this.full = entry;
    this.patches = [];
    this.ops = 0;
    this.opBytes = 0;
  }

  /** Appends a keyed patch; `false` if it is too short to have a count (it cannot be merged). */
  addPatch(value: Uint8Array): boolean {
    if (value.length < 4) return false;
    const count = (value[0] as number) | ((value[1] as number) << 8) | ((value[2] as number) << 16) | ((value[3] as number) << 24);
    this.patches.push(value);
    this.ops += count >>> 0;
    this.opBytes += value.length - 4;
    return true;
  }

  /** Whether the merged patch passes either bound a compaction keeps. */
  get oversized(): boolean {
    return this.ops > MAX_MERGED_PATCH_OPS || this.opBytes > MAX_MERGED_PATCH_BYTES;
  }

  /**
   * The patches as one keyed patch: the sum of the counts, then every patch's ops in arrival
   * order. SPEC 3.8 applies ops one after the other, each index relative to the list the previous
   * op left, so this is the same change. One patch is returned as it is.
   */
  mergedPatch(): Uint8Array {
    if (this.patches.length === 1) return this.patches[0] as Uint8Array;
    const out = new Uint8Array(4 + this.opBytes);
    const ops = this.ops >>> 0;
    out[0] = ops & 0xff;
    out[1] = (ops >>> 8) & 0xff;
    out[2] = (ops >>> 16) & 0xff;
    out[3] = (ops >>> 24) & 0xff;
    let at = 4;
    for (const patch of this.patches) {
      out.set(patch.subarray(4), at);
      at += patch.length - 4;
    }
    return out;
  }
}

/** An entry of a `no_coalesce` signal: applied as it is and announced on its own. */
class Single {
  constructor(readonly entry: ChangeEntry) {}
}

type Unit = Slot | Single;

/** A copy of `value` when it is a small view into a larger buffer, so the queue does not keep that buffer alive. */
function own(value: Uint8Array): Uint8Array {
  return value.byteLength * 2 + 64 < value.buffer.byteLength ? value.slice() : value;
}

/**
 * The host's copy of the core's store state: a registry from store handle to
 * the function that applies its signal updates, plus the queue that turns
 * arriving change-sets into one merged drain per frame (docs/SPEC.md section 11).
 *
 * ```ts
 * mirror.register(handle, (signalId, op, value) => { ... });
 * mirror.enqueue(changeSetPayload);   // from the transport; schedules a drain
 * ```
 *
 * A change-set is validated whole before any of it is queued (a change-set is
 * a transaction). A drain folds the queued entries per signal: a full value
 * supersedes everything before it, consecutive keyed patches become one patch,
 * so each signal is applied at most twice per drain (its last full value, then
 * its merged patch), signals in the order their first entry arrived, and the
 * signals a drain touched are announced once at its end. Signals a store
 * declared `no_coalesce` are applied entry by entry, each announced on its
 * own. Entries for a handle nobody registered (a store closed while updates
 * were in flight) are dropped and counted. The queue is bounded: past
 * `maxPendingEntries` or `maxPendingBytes` it is folded in place.
 */
export class Mirror {
  private readonly _registry = new Map<Handle, Registration>();
  private readonly _waiters = new Map<Handle, Waiter[]>();
  private readonly _listeners: DrainListener[] = [];
  private readonly _awaiting: Awaiting = new Map();
  private readonly _onError: (error: unknown) => void;
  private readonly _schedule: (fn: () => void) => void;
  private readonly _resync: ((handle: Handle, signalId: number) => void) | undefined;
  private readonly _maxEntries: number;
  private readonly _maxBytes: number;
  private _compactAtEntries: number;
  private _compactAtBytes: number;
  private _queue: ChangeEntry[] = [];
  private _queueBytes = 0;
  /** Change-sets and entries received since the queue was last taken by a drain. */
  private _queuedChangeSets = 0;
  private _queuedEntries = 0;
  private _resyncDue = false;
  private _scheduled = false;
  private _microtaskQueued = false;
  private _flushing = false;
  private _applied = 0;
  private _dropped = 0;
  private _changeSets = 0;
  private _entries = 0;
  private _entriesApplied = 0;
  private _drains = 0;
  private _compactions = 0;
  private _resyncs = 0;

  /** @param options See {@link MirrorOptions}. */
  constructor(options: MirrorOptions = {}) {
    this._onError =
      options.onError ??
      ((error) => {
        queueMicrotask(() => {
          throw error;
        });
      });
    // Not `queueMicrotask` itself: called as `this._schedule(...)` it would run with this mirror as
    // its receiver, and browsers throw "Illegal invocation" for a global function called on
    // anything but the window (Node does not, which is how it went unnoticed).
    this._schedule =
      options.schedule ??
      ((fn) => {
        scheduleFrame(fn);
      });
    this._resync = options.resync;
    this._maxEntries = Math.max(1, options.maxPendingEntries ?? DEFAULT_MAX_PENDING_ENTRIES);
    this._maxBytes = Math.max(1, options.maxPendingBytes ?? DEFAULT_MAX_PENDING_BYTES);
    this._compactAtEntries = this._maxEntries;
    this._compactAtBytes = this._maxBytes;
  }

  /**
   * Registers the function that applies signal updates of `handle`. A store
   * registers itself from its constructor. Throws `UndraError` (`"state"`) if
   * the handle is already registered: two stores cannot mirror one handle.
   */
  register(handle: Handle, apply: ApplyFn, options: RegisterOptions = {}): void {
    if (this._registry.has(handle)) {
      throw new UndraError("state", `handle ${String(handle)} is already registered with the mirror`);
    }
    const ids = options.noCoalesce === undefined ? null : new Set(options.noCoalesce);
    this._registry.set(handle, { apply, noCoalesce: ids !== null && ids.size > 0 ? ids : null });
  }

  /** Removes the registration of `handle` and settles the pending {@link Mirror.whenObserved} promises of it. Unknown handles are ignored. */
  unregister(handle: Handle): void {
    this._registry.delete(handle);
    this._awaiting.delete(handle);
    const waiters = this._waiters.get(handle);
    if (waiters === undefined) return;
    this._waiters.delete(handle);
    for (const w of waiters) this._settle(w, undefined);
  }

  /** Whether `handle` is registered. */
  has(handle: Handle): boolean {
    return this._registry.has(handle);
  }

  /** Number of registered handles. */
  get size(): number {
    return this._registry.size;
  }

  /** Number of entries waiting for the next drain. */
  get pending(): number {
    return this._queue.length;
  }

  /** Number of entries dropped because their handle was not registered. */
  get dropped(): number {
    return this._dropped;
  }

  /** Number of change-sets accepted so far (`stats().changeSetsReceived`). */
  get changeSets(): number {
    return this._changeSets;
  }

  /** The mirror's counters; see {@link MirrorStats}. */
  stats(): MirrorStats {
    return {
      changeSetsReceived: this._changeSets,
      entriesReceived: this._entries,
      entriesApplied: this._entriesApplied,
      drains: this._drains,
      compactions: this._compactions,
      resyncs: this._resyncs,
      pendingEntries: this._queue.length,
      pendingBytes: this._queueBytes,
      droppedEntries: this._dropped,
    };
  }

  /**
   * Calls `listener` after every drain with what it did ({@link DrainStats}).
   * Drains are timed only while a listener is registered. Returns the function
   * that removes it; a listener that throws is reported through `onError`.
   */
  addDrainListener(listener: DrainListener): () => void {
    const entry: DrainListener = (stats) => {
      listener(stats);
    };
    this._listeners.push(entry);
    return () => {
      const at = this._listeners.indexOf(entry);
      if (at >= 0) this._listeners.splice(at, 1);
    };
  }

  /**
   * Accepts a `ChangeSet` payload (SPEC 3.5): validates it whole, queues its
   * entries and schedules a drain unless one is already scheduled. A
   * malformed change-set is reported through `onError` and dropped as a whole.
   * The entries keep views into `payload`, which must not be reused.
   */
  enqueue(payload: Uint8Array): void {
    let entries: readonly ChangeEntry[];
    try {
      entries = decodeChangeSet(payload).entries;
    } catch (error) {
      this._onError(error);
      return;
    }
    this._changeSets++;
    this._queuedChangeSets++;
    this._entries += entries.length;
    this._queuedEntries += entries.length;
    if (entries.length === 0) return;
    for (const entry of entries) {
      this._queue.push(entry);
      this._queueBytes += ENTRY_OVERHEAD + entry.value.length;
    }
    if (this._queue.length > this._compactAtEntries || this._queueBytes > this._compactAtBytes) this._compact();
    // A running flush applies what arrives while it runs (see `flush`), so it needs no second one.
    if (this._flushing) return;
    // Someone waits for an initial change-set (`observe` over a worker or a socket): no frame wait.
    if (this._waiters.size > 0) this.queueFlush();
    else this._scheduleFlush();
  }

  /**
   * Flushes from a microtask queued now, so it runs before any continuation
   * queued after this call: what the runtime does before it settles a call's
   * promise, so `await store.method()` sees the change-sets that arrived
   * before the reply (docs/SPEC.md section 11). Does nothing when nothing waits.
   */
  queueFlush(): void {
    if (this._microtaskQueued || (this._queue.length === 0 && !this._resyncDue)) return;
    this._microtaskQueued = true;
    queueMicrotask(() => {
      this._microtaskQueued = false;
      this.flush();
    });
  }

  private _scheduleFlush(): void {
    if (this._scheduled) return;
    this._scheduled = true;
    this._schedule(() => {
      this._scheduled = false;
      this.flush();
    });
  }

  /**
   * Drains now: folds the queued entries per signal and applies them (see the
   * class documentation), then resolves the {@link Mirror.whenObserved}
   * promises they satisfied and calls the drain listeners. Called by the
   * scheduled drain; the runtime also calls it when a core delivers output
   * synchronously (`observe` and `callSync` in process). A nested call returns
   * at once.
   *
   * Entries queued while draining are applied by the same drain, in further
   * rounds: the subscribers of a round are notified when it ends, and one
   * that makes a synchronous core call (every synchronous method in
   * `wasm-main`) queues the change-set of that call after the round took the
   * queue. After 1000 rounds the rest is left to a later drain.
   */
  flush(): void {
    if (this._flushing || (this._queue.length === 0 && !this._resyncDue)) return;
    this._flushing = true;
    const timed = this._listeners.length > 0;
    const started = timed ? now() : 0;
    const satisfied: Waiter[] = [];
    let changeSets = 0;
    let entries = 0;
    this._applied = 0;
    try {
      for (let round = 0; this._queue.length > 0 || this._resyncDue; round++) {
        if (round === MAX_ROUNDS) {
          this._onError(
            new UndraError(
              "state",
              `the mirror applied ${MAX_ROUNDS} rounds of change-sets in one flush: a signal subscriber keeps causing changes to a store it observes`,
            ),
          );
          break;
        }
        const queued = this._queue;
        this._queue = [];
        this._queueBytes = 0;
        this._compactAtEntries = this._maxEntries;
        this._compactAtBytes = this._maxBytes;
        changeSets += this._queuedChangeSets;
        entries += this._queuedEntries;
        this._queuedChangeSets = 0;
        this._queuedEntries = 0;
        if (queued.length > 0) this._applyUnits(this._fold(queued, false), satisfied);
        this._requestResyncs();
      }
    } finally {
      this._flushing = false;
      this._entriesApplied += this._applied;
      this._drains++;
      // Left over by the round cap or by an error that unwound the loop: drained later, never stranded.
      if (this._queue.length > 0 || this._resyncDue) this._scheduleFlush();
    }
    for (const waiter of satisfied) this._settle(waiter, undefined);
    if (timed) {
      const stats: DrainStats = { changeSets, entries, appliedEntries: this._applied, durationMs: now() - started };
      for (const listener of [...this._listeners]) {
        try {
          listener(stats);
        } catch (error) {
          this._onError(error);
        }
      }
    }
  }

  /**
   * Folds `entries` (arrival order) per signal: one {@link Slot} per signal, in the order of its
   * first entry; for a drain (`everyKey` false), a {@link Single} per entry of a `no_coalesce`
   * signal, in place. A compaction folds every signal. Entries of a signal whose merged patch was
   * dropped are discarded until its next full value.
   */
  private _fold(entries: readonly ChangeEntry[], everyKey: boolean): Unit[] {
    const units: Unit[] = [];
    const slots = new Map<Handle, Map<number, Slot>>();
    const awaiting = this._awaiting;
    let lastHandle: Handle | undefined;
    let lastSlots = new Map<number, Slot>();
    let lastNoCoalesce: ReadonlySet<number> | null = null;
    let lastAwaiting: Map<number, boolean> | undefined;
    for (const entry of entries) {
      if (entry.handle !== lastHandle) {
        lastHandle = entry.handle;
        const found = slots.get(entry.handle);
        if (found === undefined) {
          lastSlots = new Map();
          slots.set(entry.handle, lastSlots);
        } else {
          lastSlots = found;
        }
        lastNoCoalesce = everyKey ? null : (this._registry.get(entry.handle)?.noCoalesce ?? null);
        lastAwaiting = awaiting.size > 0 ? awaiting.get(entry.handle) : undefined;
      }
      if (lastAwaiting?.has(entry.signalId) === true) {
        // Its patches are relative to a list this host never saw: wait for a full value.
        if (entry.op === ChangeOp.KeyedPatch) continue;
        lastAwaiting.delete(entry.signalId);
        if (lastAwaiting.size === 0) {
          awaiting.delete(entry.handle);
          lastAwaiting = undefined;
        }
      }
      if (lastNoCoalesce?.has(entry.signalId) === true) {
        units.push(new Single(entry));
        continue;
      }
      let slot = lastSlots.get(entry.signalId);
      if (slot === undefined) {
        slot = new Slot(entry.handle, entry.signalId);
        lastSlots.set(entry.signalId, slot);
        units.push(slot);
      }
      slot.entries++;
      if (entry.op !== ChangeOp.KeyedPatch) {
        slot.setFull(entry);
      } else if (!slot.addPatch(entry.value)) {
        this._onError(
          new UndraError(
            "state",
            `a keyed patch for signal ${entry.signalId} of handle ${String(entry.handle)} has no operation count; the signal is re-observed`,
          ),
        );
        this._markDropped(slot);
        lastAwaiting = awaiting.get(entry.handle);
      }
    }
    return units;
  }

  /** Forgets what `slot` holds and waits for a full value of its signal (re-observed at the next drain). */
  private _markDropped(slot: Slot): void {
    slot.full = null;
    slot.patches = [];
    slot.ops = 0;
    slot.opBytes = 0;
    let signals = this._awaiting.get(slot.handle);
    if (signals === undefined) {
      signals = new Map();
      this._awaiting.set(slot.handle, signals);
    }
    signals.set(slot.signalId, true);
    this._resyncDue = true;
  }

  /**
   * Folds the backlog in place (decision 3 of ADR-031), on the thread that passed the bound. A
   * merged patch past either patch bound is dropped and its signal re-observed at the next drain.
   * The next compaction waits until the backlog doubles, so folding stays O(1) per entry.
   */
  private _compact(): void {
    const units = this._fold(this._queue, true);
    const queue: ChangeEntry[] = [];
    let bytes = 0;
    for (const unit of units) {
      if (unit instanceof Single) continue; // a compaction folds every signal
      if (unit.oversized) {
        this._markDropped(unit);
        continue;
      }
      if (unit.full !== null) {
        const value = own(unit.full.value);
        queue.push({ handle: unit.handle, signalId: unit.signalId, op: unit.full.op, value });
        bytes += ENTRY_OVERHEAD + value.length;
      }
      if (unit.patches.length > 0) {
        const value = own(unit.mergedPatch());
        queue.push({ handle: unit.handle, signalId: unit.signalId, op: ChangeOp.KeyedPatch, value });
        bytes += ENTRY_OVERHEAD + value.length;
      }
    }
    this._queue = queue;
    this._queueBytes = bytes;
    this._compactions++;
    this._compactAtEntries = Math.max(this._maxEntries, 2 * queue.length);
    this._compactAtBytes = Math.max(this._maxBytes, 2 * bytes);
  }

  /**
   * Applies folded units in order. Consecutive slots share one signal batch; a `no_coalesce`
   * entry gets a batch of its own, so its subscribers hear every value. If a notification throws
   * (a signal error handler that rethrows), the units not applied yet go back to the front of the
   * queue for a later drain.
   */
  private _applyUnits(units: readonly Unit[], satisfied: Waiter[]): void {
    let next = 0;
    try {
      while (next < units.length) {
        const first = units[next] as Unit;
        if (first instanceof Single) {
          next++;
          batch(() => {
            this._applyEntry(first.entry, satisfied);
          });
          continue;
        }
        batch(() => {
          while (next < units.length) {
            const unit = units[next] as Unit;
            if (unit instanceof Single) break;
            next++;
            this._applySlot(unit, satisfied);
          }
        });
      }
    } catch (error) {
      if (next < units.length) this._requeue(units, next);
      throw error;
    }
  }

  private _requeue(units: readonly Unit[], from: number): void {
    const older: ChangeEntry[] = [];
    for (let i = from; i < units.length; i++) {
      const unit = units[i] as Unit;
      if (unit instanceof Single) {
        older.push(unit.entry);
        continue;
      }
      if (unit.full !== null) older.push(unit.full);
      if (unit.patches.length > 0) {
        older.push({ handle: unit.handle, signalId: unit.signalId, op: ChangeOp.KeyedPatch, value: unit.mergedPatch() });
      }
    }
    for (const entry of older) this._queueBytes += ENTRY_OVERHEAD + entry.value.length;
    this._queue = older.concat(this._queue);
  }

  private _applySlot(slot: Slot, satisfied: Waiter[]): void {
    const registration = this._registry.get(slot.handle);
    if (registration === undefined) {
      this._dropped += slot.entries;
      return;
    }
    if (slot.full === null && slot.patches.length === 0) return; // dropped for a resync
    if (slot.full !== null) this._call(registration, slot.signalId, slot.full.op, slot.full.value);
    if (slot.patches.length > 0) this._call(registration, slot.signalId, ChangeOp.KeyedPatch, slot.mergedPatch());
    this._satisfy(slot.handle, slot.signalId, satisfied);
  }

  private _applyEntry(entry: ChangeEntry, satisfied: Waiter[]): void {
    const registration = this._registry.get(entry.handle);
    if (registration === undefined) {
      this._dropped++;
      return;
    }
    this._call(registration, entry.signalId, entry.op, entry.value);
    this._satisfy(entry.handle, entry.signalId, satisfied);
  }

  private _call(registration: Registration, signalId: number, op: ChangeOp, value: Uint8Array): void {
    this._applied++;
    try {
      registration.apply(signalId, op, value);
    } catch (error) {
      this._onError(error);
    }
  }

  private _satisfy(handle: Handle, signalId: number, satisfied: Waiter[]): void {
    const waiters = this._waiters.get(handle);
    if (waiters === undefined) return;
    const rest: Waiter[] = [];
    for (const w of waiters) {
      if (w.signalId === ALL_SIGNALS || w.signalId === signalId) satisfied.push(w);
      else rest.push(w);
    }
    if (rest.length === 0) this._waiters.delete(handle);
    else this._waiters.set(handle, rest);
  }

  /** Re-observes the signals whose merged patch was dropped, once each, from the drain. */
  private _requestResyncs(): void {
    if (!this._resyncDue) return;
    this._resyncDue = false;
    for (const [handle, signals] of [...this._awaiting]) {
      if (!this._registry.has(handle)) {
        this._awaiting.delete(handle);
        continue;
      }
      for (const [signalId, due] of [...signals]) {
        if (!due) continue;
        signals.set(signalId, false);
        if (this._resync === undefined) continue;
        this._resyncs++;
        try {
          this._resync(handle, signalId);
        } catch (error) {
          this._onError(error);
        }
      }
    }
  }

  /**
   * A promise that resolves once an entry for `signalId` of `handle` (any
   * signal of it for `ALL_SIGNALS`) has been applied by a drain and its
   * subscribers have been notified: the "initial change-set" barrier behind
   * `UndraCore.observe`. It also resolves when the handle is unregistered, and
   * rejects with `UndraError("observe")` after `timeoutMs` (never, for 0).
   * While a promise waits, change-sets are drained without waiting for a frame.
   */
  whenObserved(handle: Handle, signalId: number, timeoutMs = 0): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const waiter: Waiter = { signalId, resolve, reject, timer: undefined };
      if (timeoutMs > 0) {
        waiter.timer = setTimeout(() => {
          this._remove(handle, waiter);
          reject(
            new UndraError(
              "observe",
              `no change-set arrived for signal ${signalId === ALL_SIGNALS ? "*" : String(signalId)} of handle ${String(handle)} within ${timeoutMs} ms; is the handle a live store?`,
            ),
          );
        }, timeoutMs);
      }
      const list = this._waiters.get(handle);
      if (list === undefined) this._waiters.set(handle, [waiter]);
      else list.push(waiter);
      // Entries that arrived before the promise was made are drained now, not at the next frame.
      this.queueFlush();
    });
  }

  /** Rejects every pending {@link Mirror.whenObserved} promise with `error` (the transport went away). */
  failWaiters(error: unknown): void {
    const all = [...this._waiters.values()].flat();
    this._waiters.clear();
    for (const w of all) this._settle(w, { error });
  }

  private _remove(handle: Handle, waiter: Waiter): void {
    const list = this._waiters.get(handle);
    if (list === undefined) return;
    const rest = list.filter((w) => w !== waiter);
    if (rest.length === 0) this._waiters.delete(handle);
    else this._waiters.set(handle, rest);
  }

  private _settle(waiter: Waiter, failure: { readonly error: unknown } | undefined): void {
    if (waiter.timer !== undefined) clearTimeout(waiter.timer);
    waiter.timer = undefined;
    if (failure === undefined) waiter.resolve();
    else waiter.reject(failure.error);
  }
}
