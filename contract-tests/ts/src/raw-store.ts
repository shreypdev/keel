import { ALL_SIGNALS, CallTarget, type ChangeOp, type Codec, type KeelCore, KeelWriter, decodeValue, encodeValue } from "@keel/runtime";

/** One entry of a change-set as the mirror hands it to a store. */
export interface SignalUpdate {
  /** Index of the signal within the store. */
  readonly signalId: number;
  /** 0 full value, 1 keyed patch, 2 lazy list invalidated. */
  readonly op: ChangeOp;
  /** The encoded value or patch (a copy: safe to keep). */
  readonly value: Uint8Array;
}

/** The ids a constructor needs: the `typeId` and `new` of an object in `KeelIds.Objects`. */
export interface ConstructorIds {
  readonly typeId: number;
  readonly new: number;
}

/** What {@link RawStore.open} can be told. */
export interface OpenOptions {
  /** The constructor's encoded arguments. Default none. */
  readonly args?: Uint8Array;
  /** Observe every signal straight away (the default). `false` leaves the store constructed but unobserved. */
  readonly observe?: boolean;
}

const NO_ARGS = new Uint8Array(0);

/**
 * A store used without its generated class: the scenario constructs it, registers its own mirror
 * callback, observes it, and looks at the raw change-set entries the core delivers (SPEC 17.1:
 * `core.construct`, `core.mirror.register`, `core.observe`). This is how a scenario counts entries
 * and checks op codes, which a generated store hides behind its signals.
 *
 * ```ts
 * const counter = await RawStore.open(core, KeelIds.Objects.Counter);
 * await counter.call(KeelIds.Objects.Counter.add, encodeValue(codecs.i32, 5));
 * counter.take(); // the entries of the initial change-set, then of the add
 * ```
 */
export class RawStore {
  /** Opens a store: constructs it, registers the recording mirror callback and (by default) observes all its signals. */
  static async open(core: KeelCore, ids: ConstructorIds, options: OpenOptions = {}): Promise<RawStore> {
    const handle = await core.construct(ids.typeId, ids.new, options.args ?? NO_ARGS);
    const store = new RawStore(core, handle);
    if (options.observe !== false) await store.observe(true);
    return store;
  }

  /** The core the store lives in. */
  readonly core: KeelCore;
  /** The store's handle. */
  readonly handle: bigint;
  #entries: SignalUpdate[] = [];
  #closed = false;

  private constructor(core: KeelCore, handle: bigint) {
    this.core = core;
    this.handle = handle;
    core.mirror.register(handle, (signalId, op, value) => {
      this.#entries.push({ signalId, op, value: value.slice() });
    });
  }

  /** Starts or stops observing every signal; resolves once the initial change-set (if any) has been delivered. */
  observe(on: boolean): Promise<void> {
    return this.core.observe(this.handle, ALL_SIGNALS, on);
  }

  /** Calls a method of the object and resolves with the encoded result. */
  call(methodId: number, args: Uint8Array = NO_ARGS, signal?: AbortSignal): Promise<Uint8Array> {
    return this.core.call({ target: CallTarget.ObjectMethod, handle: this.handle }, methodId, args, signal);
  }

  /** Calls a method and decodes its result with `codec`. */
  async callDecoded<T>(methodId: number, codec: Codec<T>, args: Uint8Array = NO_ARGS): Promise<T> {
    return decodeValue(codec, await this.call(methodId, args));
  }

  /** Calls a method with `args` encoded by `codec`, for methods with one argument. */
  callWith<A>(methodId: number, codec: Codec<A>, arg: A): Promise<Uint8Array> {
    return this.call(methodId, encodeValue(codec, arg));
  }

  /** How many entries the mirror has handed to this store since the last `take()`, without flushing: what has arrived, not what could. */
  get received(): number {
    return this.#entries.length;
  }

  /**
   * The entries delivered since the last `take()` (or since `open`), oldest first, and forgets
   * them. Flushes the mirror first, so entries the core has already sent are included without
   * waiting for the runtime's scheduled flush.
   */
  take(): SignalUpdate[] {
    this.core.mirror.flush();
    const entries = this.#entries;
    this.#entries = [];
    return entries;
  }

  /** Releases the store's handle and stops recording. Idempotent. */
  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.core.release(this.handle);
  }
}

/**
 * The encoded arguments of a call: what a generated method writes before it calls the core.
 *
 * ```ts
 * store.call(BigList.insertAt, args((w) => { w.writeU32(5000); w.writeStr("fresh"); }));
 * ```
 */
export function args(write: (w: KeelWriter) => void): Uint8Array {
  const w = new KeelWriter();
  write(w);
  return w.finish();
}

/** The entry for `signalId` among `entries` (the last one if there are several), decoded with `codec`. Fails if there is none. */
export function valueOf<T>(entries: readonly SignalUpdate[], signalId: number, codec: Codec<T>): T {
  for (let i = entries.length - 1; i >= 0; i--) {
    const entry = entries[i] as SignalUpdate;
    if (entry.signalId === signalId) return decodeValue(codec, entry.value);
  }
  throw new Error(`no entry for signal ${signalId} among [${entries.map((e) => e.signalId).join(", ")}]`);
}
