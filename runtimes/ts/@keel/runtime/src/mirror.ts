import { KeelError } from "./errors.js";
import { batch } from "./signal.js";
import { ALL_SIGNALS, type ChangeEntry, type ChangeOp, type Handle, decodeChangeSet } from "./wire/index.js";

/** Applies one signal update to the store registered for a handle (`KeelStore._apply`). */
export type ApplyFn = (signalId: number, op: ChangeOp, value: Uint8Array) => void;

/** Options of a {@link Mirror}. */
export interface MirrorOptions {
  /** Receives every failure the mirror cannot report to a caller: a malformed change-set, an `apply` function that throws. Default: rethrow from a microtask. */
  readonly onError?: (error: unknown) => void;
  /** Schedules the flush of queued change-sets. Default `queueMicrotask`, which gives one flush per macrotask. */
  readonly schedule?: (fn: () => void) => void;
}

interface Waiter {
  readonly signalId: number;
  readonly resolve: () => void;
  readonly reject: (error: unknown) => void;
  timer: ReturnType<typeof setTimeout> | undefined;
}

/**
 * The host's copy of the core's store state: a registry from store handle to
 * the function that applies its signal updates, plus the queue that turns
 * arriving change-sets into one `flush()` per macrotask.
 *
 * ```ts
 * mirror.register(handle, (signalId, op, value) => { ... });
 * mirror.enqueue(changeSetPayload);   // from the transport; schedules a flush
 * ```
 *
 * A change-set is decoded and validated whole before any of it is queued
 * (a change-set is a transaction), queued entries are applied in arrival
 * order, and the signals they touched are announced once at the end of the
 * flush. Entries for a handle nobody registered (a store closed while
 * updates were in flight) are dropped and counted.
 */
export class Mirror {
  readonly #registry = new Map<Handle, ApplyFn>();
  readonly #waiters = new Map<Handle, Waiter[]>();
  readonly #onError: (error: unknown) => void;
  readonly #schedule: (fn: () => void) => void;
  #queue: ChangeEntry[] = [];
  #scheduled = false;
  #flushing = false;
  #dropped = 0;
  #changeSets = 0;

  /** @param options See {@link MirrorOptions}. */
  constructor(options: MirrorOptions = {}) {
    this.#onError =
      options.onError ??
      ((error) => {
        queueMicrotask(() => {
          throw error;
        });
      });
    // Not `queueMicrotask` itself: called as `this.#schedule(...)` it would run with this mirror as
    // its receiver, and browsers throw "Illegal invocation" for a global function called on
    // anything but the window (Node does not, which is how it went unnoticed).
    this.#schedule =
      options.schedule ??
      ((fn) => {
        queueMicrotask(fn);
      });
  }

  /**
   * Registers the function that applies signal updates of `handle`. A store
   * registers itself from its constructor. Throws `KeelError` (`"state"`) if
   * the handle is already registered: two stores cannot mirror one handle.
   */
  register(handle: Handle, apply: ApplyFn): void {
    if (this.#registry.has(handle)) {
      throw new KeelError("state", `handle ${String(handle)} is already registered with the mirror`);
    }
    this.#registry.set(handle, apply);
  }

  /** Removes the registration of `handle` and settles the pending {@link Mirror.whenObserved} promises of it. Unknown handles are ignored. */
  unregister(handle: Handle): void {
    this.#registry.delete(handle);
    const waiters = this.#waiters.get(handle);
    if (waiters === undefined) return;
    this.#waiters.delete(handle);
    for (const w of waiters) this.#settle(w, undefined);
  }

  /** Whether `handle` is registered. */
  has(handle: Handle): boolean {
    return this.#registry.has(handle);
  }

  /** Number of registered handles. */
  get size(): number {
    return this.#registry.size;
  }

  /** Number of entries waiting for the next flush. */
  get pending(): number {
    return this.#queue.length;
  }

  /** Number of entries dropped because their handle was not registered. */
  get dropped(): number {
    return this.#dropped;
  }

  /** Number of change-sets accepted so far. */
  get changeSets(): number {
    return this.#changeSets;
  }

  /**
   * Accepts a `ChangeSet` payload (SPEC 3.5): validates it whole, queues its
   * entries and schedules a flush unless one is already scheduled. A
   * malformed change-set is reported through `onError` and dropped as a whole.
   * The entries keep views into `payload`, which must not be reused.
   */
  enqueue(payload: Uint8Array): void {
    let entries: readonly ChangeEntry[];
    try {
      entries = decodeChangeSet(payload).entries;
    } catch (error) {
      this.#onError(error);
      return;
    }
    this.#changeSets++;
    if (entries.length === 0) return;
    for (const entry of entries) this.#queue.push(entry);
    if (!this.#scheduled && !this.#flushing) {
      this.#scheduled = true;
      this.#schedule(() => {
        this.#scheduled = false;
        this.flush();
      });
    }
  }

  /**
   * Applies every queued entry now, in arrival order, inside one signal
   * {@link batch}, then resolves the {@link Mirror.whenObserved} promises the
   * entries satisfied. Called by the scheduled flush; the runtime also calls
   * it directly when a core delivers output synchronously (`observe` in
   * process). Entries queued while flushing are applied by the same flush.
   * A nested call returns at once.
   */
  flush(): void {
    if (this.#flushing || this.#queue.length === 0) return;
    this.#flushing = true;
    const satisfied: Waiter[] = [];
    try {
      batch(() => {
        while (this.#queue.length > 0) {
          const entries = this.#queue;
          this.#queue = [];
          for (const entry of entries) this.#apply(entry, satisfied);
        }
      });
    } finally {
      this.#flushing = false;
    }
    for (const waiter of satisfied) this.#settle(waiter, undefined);
  }

  #apply(entry: ChangeEntry, satisfied: Waiter[]): void {
    const apply = this.#registry.get(entry.handle);
    if (apply === undefined) {
      this.#dropped++;
      return;
    }
    try {
      apply(entry.signalId, entry.op, entry.value);
    } catch (error) {
      this.#onError(error);
    }
    const waiters = this.#waiters.get(entry.handle);
    if (waiters === undefined) return;
    const rest: Waiter[] = [];
    for (const w of waiters) {
      if (w.signalId === ALL_SIGNALS || w.signalId === entry.signalId) satisfied.push(w);
      else rest.push(w);
    }
    if (rest.length === 0) this.#waiters.delete(entry.handle);
    else this.#waiters.set(entry.handle, rest);
  }

  /**
   * A promise that resolves once an entry for `signalId` of `handle` (any
   * signal of it for `ALL_SIGNALS`) has been applied by a flush and its
   * subscribers have been notified: the "initial change-set" barrier behind
   * `KeelCore.observe`. It also resolves when the handle is unregistered, and
   * rejects with `KeelError("observe")` after `timeoutMs` (never, for 0).
   */
  whenObserved(handle: Handle, signalId: number, timeoutMs = 0): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const waiter: Waiter = { signalId, resolve, reject, timer: undefined };
      if (timeoutMs > 0) {
        waiter.timer = setTimeout(() => {
          this.#remove(handle, waiter);
          reject(
            new KeelError(
              "observe",
              `no change-set arrived for signal ${signalId === ALL_SIGNALS ? "*" : String(signalId)} of handle ${String(handle)} within ${timeoutMs} ms; is the handle a live store?`,
            ),
          );
        }, timeoutMs);
      }
      const list = this.#waiters.get(handle);
      if (list === undefined) this.#waiters.set(handle, [waiter]);
      else list.push(waiter);
    });
  }

  /** Rejects every pending {@link Mirror.whenObserved} promise with `error` (the transport went away). */
  failWaiters(error: unknown): void {
    const all = [...this.#waiters.values()].flat();
    this.#waiters.clear();
    for (const w of all) this.#settle(w, { error });
  }

  #remove(handle: Handle, waiter: Waiter): void {
    const list = this.#waiters.get(handle);
    if (list === undefined) return;
    const rest = list.filter((w) => w !== waiter);
    if (rest.length === 0) this.#waiters.delete(handle);
    else this.#waiters.set(handle, rest);
  }

  #settle(waiter: Waiter, failure: { readonly error: unknown } | undefined): void {
    if (waiter.timer !== undefined) clearTimeout(waiter.timer);
    waiter.timer = undefined;
    if (failure === undefined) waiter.resolve();
    else waiter.reject(failure.error);
  }
}
