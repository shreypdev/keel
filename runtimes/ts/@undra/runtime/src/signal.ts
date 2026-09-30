/*
 * Signals: the host-side value of a core signal (docs/SPEC.md sections 10.3
 * and 17.1). A `Signal<T>` holds the latest decoded value and tells its
 * subscribers when it changed. The mirror applies a whole flush of
 * change-sets inside `batch`, so a subscriber hears about a signal once per
 * flush, with the final value, no matter how many change-sets touched it.
 */

/** Called with the new value of a signal after it changed. */
export type Subscriber<T> = (value: T) => void;

/** Handles an exception thrown by a subscriber; see {@link setSignalErrorHandler}. */
export type SignalErrorHandler = (error: unknown) => void;

function rethrowAsync(error: unknown): void {
  queueMicrotask(() => {
    throw error;
  });
}

let errorHandler: SignalErrorHandler = rethrowAsync;

/**
 * Replaces the handler for exceptions thrown by subscribers and returns the
 * previous one. A subscriber that throws never stops the others from being
 * notified; the default handler rethrows the error from a microtask so it
 * surfaces as an uncaught exception.
 */
export function setSignalErrorHandler(handler: SignalErrorHandler): SignalErrorHandler {
  const previous = errorHandler;
  errorHandler = handler;
  return previous;
}

/** Signals whose value changed inside the current batch and that have subscribers. */
const dirty: Array<{ _notify(): void }> = [];
let batchDepth = 0;

/** Signals read through `get()` while a {@link trackReads} call is running. */
let collector: Set<Signal<unknown>> | null = null;

/**
 * Runs `fn` with subscriber notification deferred to the end of the outermost
 * batch: a signal that changes several times inside is announced once, with
 * its final value. The mirror wraps every flush in a batch; call it yourself
 * to group `_set` calls made outside a flush. Batches nest.
 */
export function batch<R>(fn: () => R): R {
  batchDepth++;
  try {
    return fn();
  } finally {
    if (--batchDepth === 0) drain();
  }
}

function drain(): void {
  // Notification may re-enter (a subscriber that sets a signal): the loop
  // picks such signals up in the same drain instead of recursing.
  batchDepth++;
  try {
    for (let i = 0; i < dirty.length; i++) (dirty[i] as { _notify(): void })._notify();
  } finally {
    dirty.length = 0;
    batchDepth--;
  }
}

/**
 * Runs `fn` and returns its result together with every signal it read through
 * `get()` (not `peek()`). Framework adapters and derived state build on this.
 */
export function trackReads<R>(fn: () => R): { readonly value: R; readonly signals: ReadonlySet<Signal<unknown>> } {
  const previous = collector;
  const seen = new Set<Signal<unknown>>();
  collector = seen;
  try {
    const value = fn();
    return { value, signals: seen };
  } finally {
    collector = previous;
  }
}

/**
 * The host-side value of one core signal.
 *
 * ```ts
 * const count = new Signal(0);
 * const stop = count.subscribe((n) => console.log(n));
 * count._set(1); // logs 1
 * stop();
 * ```
 *
 * Generated stores expose their signals as `readonly` fields and update them
 * from change-sets with `_set`; application code only reads and subscribes.
 */
export class Signal<T> {
  #value: T;
  /**
   * Typed `Subscriber<never>`, not `Subscriber<T>`: a `#private` field takes
   * part in assignability, and a `Signal<number>` must stay assignable to
   * `Signal<unknown>` (generated stores list their signals that way).
   */
  #subscribers: Set<{ readonly fn: Subscriber<never> }> | null = null;
  #queued = false;

  /** @param initial The placeholder value shown until the first change-set arrives. */
  constructor(initial: T) {
    this.#value = initial;
  }

  /**
   * The current value. Reading through `get()` registers the signal with an
   * enclosing {@link trackReads}; use {@link Signal.peek} to read without that.
   */
  get(): T {
    collector?.add(this as Signal<unknown>);
    return this.#value;
  }

  /** The current value, without dependency tracking. The reference is stable until the signal changes. */
  peek(): T {
    return this.#value;
  }

  /**
   * Calls `fn` with the new value each time the signal changes (not with the
   * current value). Returns a function that removes the subscription; calling
   * it twice is harmless. The same function may be subscribed more than once.
   */
  subscribe(fn: Subscriber<T>): () => void {
    const entry: { readonly fn: Subscriber<never> } = { fn };
    (this.#subscribers ??= new Set()).add(entry);
    return () => {
      this.#subscribers?.delete(entry);
    };
  }

  /** Number of active subscriptions. */
  get subscriberCount(): number {
    return this.#subscribers?.size ?? 0;
  }

  /**
   * Internal: replaces the value, as the mirror does when a change-set
   * arrives. A value equal to the current one (`Object.is`) is ignored.
   * Subscribers are notified at the end of the enclosing {@link batch}, or
   * immediately when there is none.
   */
  _set(value: T): void {
    if (Object.is(value, this.#value)) return;
    this.#value = value;
    if (this.#subscribers === null || this.#subscribers.size === 0) return;
    if (batchDepth > 0) {
      if (!this.#queued) {
        this.#queued = true;
        dirty.push(this);
      }
    } else {
      this._notify();
    }
  }

  /** Internal: announces the current value to the subscribers present now. */
  _notify(): void {
    this.#queued = false;
    const subscribers = this.#subscribers;
    if (subscribers === null || subscribers.size === 0) return;
    const value = this.#value;
    // A copy, so that a subscriber may unsubscribe (or subscribe) while we iterate.
    for (const entry of [...subscribers]) {
      if (!subscribers.has(entry)) continue;
      try {
        (entry.fn as Subscriber<T>)(value);
      } catch (error) {
        errorHandler(error);
      }
    }
  }
}
