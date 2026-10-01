import { UndraCallError } from "./call-error.js";
import type { UndraCore } from "./core.js";
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
          leak.core.deref()?.release(leak.handle);
        } catch {
          // The core is closed or going away; there is nothing left to release.
        }
      })
    : null;

/**
 * A core object addressed by handle: the base class of generated objects and
 * stores. Owns exactly one handle; `close()` (idempotent) releases it.
 *
 * ```ts
 * using calc = await Calculator.create();
 * await calc.add(1, 2);
 * ```
 */
export abstract class UndraObject {
  /** The core this object lives in. */
  readonly core: UndraCore;
  /** The handle of the object inside the core. */
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

/** What a generated store tells its base class about itself. */
export interface StoreOptions {
  /**
   * The ids of the signals declared `#[undra(no_coalesce)]`: the mirror applies and announces
   * every value of these instead of the last one per frame (docs/SPEC.md section 11).
   */
  readonly noCoalesce?: readonly number[];
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
