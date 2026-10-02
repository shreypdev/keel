import { UndraCallError } from "./call-error.js";
import type { UndraCore } from "./core.js";
import { collected } from "./identity.js";
import type { Signal } from "./signal.js";
import { ALL_SIGNALS, type ChangeOp, type Handle } from "./wire/index.js";

/**
 * `Symbol.dispose` where the platform has it (Node 20+, current browsers),
 * else the well-known registry symbol that polyfills and TypeScript's
 * down-levelled `using` agree on.
 */
const DISPOSE: typeof Symbol.dispose = Symbol.dispose ?? (Symbol.for("Symbol.dispose") as typeof Symbol.dispose);

/** What the finalizer needs to release a leaked handle. It must not reference the object itself. */
interface Leak {
  readonly core: WeakRef<UndraCore>;
  readonly handle: Handle;
}

/**
 * Backstop for objects that were dropped without `close()`: once the object
 * is garbage-collected the handle is released. Not deterministic; close
 * objects explicitly (or with `using`).
 */
const leaks: FinalizationRegistry<Leak> | null =
  typeof FinalizationRegistry === "function"
    ? new FinalizationRegistry<Leak>((leak) => {
        try {
          const core = leak.core.deref();
          if (core !== undefined) collected(core, leak.handle);
        } catch {
          // The core is closed or going away; there is nothing left to release.
        }
      })
    : null;

/**
 * A core object addressed by handle: the base class of generated objects and
 * stores. Owns exactly one reference to its handle; `close()` (idempotent)
 * releases it. There is one wrapper per handle (`adopt`, ADR-040), so `===`
 * between wrappers is identity of the core's objects.
 *
 * ```ts
 * using calc = await Calculator.create();
 * await calc.add(1, 2);
 * ```
 */
export abstract class UndraObject {
  /** The core this object lives in. */
  readonly core: UndraCore;
  /**
   * The handle of the object inside the core. It changes only when crash recovery re-creates a query handle
   * (ADR-049): code that keeps the raw handle instead of the object goes stale then.
   */
  readonly handle: Handle;
  #closed = false;

  /** @param core The core that issued `handle`. @param handle A live handle the caller owns and hands over. */
  protected constructor(core: UndraCore, handle: Handle) {
    this.core = core;
    this.handle = handle;
    leaks?.register(this, { core: new WeakRef(core), handle }, this);
  }

  /** Whether `close()` has been called. */
  get closed(): boolean {
    return this.#closed;
  }

  /** Releases the handle. Later calls on the object fail in the core with a stale handle. Idempotent. */
  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    leaks?.unregister(this);
    this.core.release(this.handle);
  }

  /** `using` support: same as {@link UndraObject.close}. */
  [DISPOSE](): void {
    this.close();
  }
}

/**
 * Moves `object` to `handle`, a new object crash recovery created in the core in its place (a re-created query handle,
 * ADR-049); the handle it had is not released (the instance that issued it is gone).
 *
 * @internal Used by `crashRecovery` only.
 */
export function _rebindObject(object: UndraObject, handle: Handle): void {
  leaks?.unregister(object);
  (object as { handle: Handle }).handle = handle;
  leaks?.register(object, { core: new WeakRef(object.core), handle }, object);
}

/** What a generated store tells its base class about itself. */
export interface StoreOptions {
  /**
   * The ids of the signals declared `#[undra(no_coalesce)]`: the mirror applies and announces
   * every value of these instead of the last one per frame (docs/SPEC.md section 11).
   */
  readonly noCoalesce?: readonly number[];
  /**
   * The constructor call that made this store, for a store the runtime re-creates after a crash recovery instead of
   * restoring it (ADR-049): a query handle, which a snapshot leaves out. Generated query handles pass it.
   */
  readonly recreate?: RecreateCall;
}

/** A recorded constructor call: what `UndraCore.construct` was given (ADR-049, re-created query handles). */
export interface RecreateCall {
  /** The object type. */
  readonly typeId: number;
  /** The constructor. */
  readonly methodId: number;
  /** The encoded arguments. */
  readonly args: Uint8Array;
}

/**
 * A core store: an object whose signals the host mirrors. Generated stores
 * declare one `Signal` per core signal, list them in `_signals` (in signal-id
 * order) and implement `_apply` to decode change-set entries into them.
 *
 * The constructor registers the store with the core's mirror, so updates
 * flow as soon as the generated `create()` observes it. The registration
 * holds the store weakly: an abandoned store can be collected and its handle
 * released by the finalizer.
 */
export abstract class UndraStore extends UndraObject {
  /** The store's signals in signal-id order; set by the generated constructor. */
  protected _signals: Signal<unknown>[] = [];

  /**
   * @param core The core that issued `handle`. @param handle The store's handle.
   * @param options The store's `no_coalesce` signals, when it has any (generated code passes them).
   */
  protected constructor(core: UndraCore, handle: Handle, options: StoreOptions = {}) {
    super(core, handle);
    const ref = new WeakRef(this);
    core.mirror.register(
      handle,
      (signalId, op, value) => {
        ref.deref()?._apply(signalId, op, value);
      },
      options.noCoalesce === undefined ? {} : { noCoalesce: options.noCoalesce },
    );
    if (options.recreate !== undefined) core._recreatable(this, options.recreate);
  }

  /**
   * Decodes one change-set entry into the signal `signalId` (implemented by
   * generated code): `op` says whether `value` is a full value, a keyed
   * patch, or a lazy-list invalidation.
   */
  protected abstract _apply(signalId: number, op: ChangeOp, value: Uint8Array): void;

  /**
   * Starts observing every signal, so the core reports their current values: resolves once they have been
   * applied. Generated `create()` functions call it after constructing the store. If the core cannot be
   * reached the store is closed (no handle leaks) and the failure is thrown as an `UndraCallError`.
   *
   * @throws {UndraCallError} If the core is closed or unreachable, or never delivers the initial values.
   */
  protected async _observeAll(): Promise<void> {
    try {
      await this.core.observe(this.handle, ALL_SIGNALS, true);
    } catch (error) {
      this.close();
      throw UndraCallError.mapped(error);
    }
  }

  /** Stops mirroring, then releases the handle. Idempotent. */
  override close(): void {
    if (this.closed) return;
    this.core.mirror.unregister(this.handle);
    super.close();
  }
}
