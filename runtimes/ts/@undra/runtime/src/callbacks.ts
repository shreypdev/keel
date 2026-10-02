import { UndraError } from "./base-error.js";
import type { UndraCore } from "./core.js";
import { UndraPortError, UndraReplyError } from "./errors.js";
import type { PortImpl } from "./port.js";
import { QUIETLY_UNAVAILABLE } from "./port-dispatch.js";
import { ReplyStatus, UndraReader } from "./wire/index.js";

/*
 * Host callback interfaces (ADR-041): `#[undra::callback]` traits the app implements and passes into the core. A
 * callback crosses as an instance handle the host chooses; the core calls an instance through the trait's port
 * (`port_id`, the instance first in the arguments). This module is the host side, per core:
 *
 * * the registry (`callbacks(core)`): instance -> (implementation, count), one reference per crossing, interned by
 *   object identity, so the same listener passed twice is one instance counted twice; the core gives each reference
 *   back with the reserved `__release` method, and the entry goes at zero;
 * * the bridge: one `PortImpl` per interface, registered with the core the first time one of its instances is lent.
 *   It only queues and returns: the app's code never runs inside the core's callback. `main` delivery queues the
 *   invocation in the mirror (in arrival order with the change-sets, so a listener sees the stores as they were when
 *   the core called it); `background` delivery runs it from a microtask, in call order. An `async` method answers
 *   through the port reply: its value, its own `E` (status 1), or "unavailable" (status 2) after any other failure is
 *   reported to `onError`; `__cancel` aborts the `AbortSignal` the method was given (or drops an invocation that has
 *   not started).
 *
 * Generated code only imports this module when the schema has callbacks, so an app without them does not ship it.
 */

/** The arguments of a callback method after the instance handle, decoded: the invocation, ready to run later. */
export type CallbackNotify<T> = (impl: T) => unknown;
/** The invocation of an `async` callback method: runs the implementation and encodes its answer. */
export type CallbackCall<T> = (impl: T, signal: AbortSignal) => Promise<Uint8Array>;

/** One method of a callback interface, as its generated bridge describes it. */
export type CallbackMethod<T> =
  | {
      /** The method's name as TypeScript spells it: reports name the failure `Reporter.note`. */
      readonly name: string;
      /** Keep only the newest pending invocation per instance (`#[undra(coalesce)]`). */
      readonly coalesce?: boolean;
      /** A fire-and-forget method: decodes its arguments. What the implementation throws is reported, nothing more. */
      readonly notify: (args: UndraReader) => CallbackNotify<T>;
    }
  | {
      /** The method's name as TypeScript spells it. */
      readonly name: string;
      /**
       * An `async` method: decodes its arguments. The invocation encodes the implementation's value, or throws
       * `UndraPortError` with its encoded `E`.
       */
      readonly call: (args: UndraReader) => CallbackCall<T>;
    };

/** A host callback interface (`#[undra::callback]`) as its generated bridge (`<Name>Callback`) describes it. */
export interface CallbackInterface<T extends object> {
  /** The interface's name (`"Reporter"`). */
  readonly name: string;
  /** Its port id (`fnv1a32("port.<Trait>")`). */
  readonly portId: number;
  /** The reserved `__release` method's id: the core gives a reference back. */
  readonly releaseInstance: number;
  /** The reserved `__cancel` method's id: the core dropped an `async` call. */
  readonly cancelCall: number;
  /** `#[undra::callback(background)]`: run from a microtask instead of the mirror's drain. */
  readonly background?: boolean;
  /** The methods, by method id. */
  readonly methods: Readonly<Record<number, CallbackMethod<T>>>;
}

/** What `lending` hands its `send` function: lends a callback for the call being encoded and returns its instance handle. */
export type Lend = <T extends object>(impl: T, callback: CallbackInterface<T>) => bigint;

/** One lent implementation. */
interface Lent {
  readonly impl: object;
  readonly callback: CallbackInterface<object>;
  count: number;
}

/** An `async` invocation, by port call id, until it answers. */
interface Running {
  readonly controller: AbortController;
}

/** What a superseded `coalesce` invocation sees. */
interface Pending {
  superseded: boolean;
}

/** The rejection of a weak wrapper's `async` method once its target is gone: "unavailable", nothing to report. */
const GONE = new UndraError("state", "the callback's target was garbage-collected");

/** A promise that never settles: the answer of a call that expects no reply (the bridge returns 1, "later", and never replies). */
function noReply(): Promise<Uint8Array> {
  return new Promise<Uint8Array>(() => {});
}

/**
 * The host callbacks of one core (ADR-041): the implementations the core holds references to, by instance handle.
 * `callbacks(core)` returns it. Strong: an implementation stays alive while the core holds a reference to it (use the
 * generated `weak<Name>(target)` wrapper to break a cycle).
 */
export class UndraCallbacks {
  readonly #core: UndraCore;
  #next = 0n;
  readonly #lent = new Map<bigint, Lent>();
  readonly #instances = new Map<CallbackInterface<object>, WeakMap<object, bigint>>();
  readonly #bridged = new Set<number>();
  readonly #running = new Map<number, Running>();
  readonly #coalesced = new Map<string, Pending>();
  /**
   * The highest instance a drop forgot (a lost connection, a closed core, a restart after a trap): a later
   * `__release` of one of those is expected, not an over-release. Instances lent after it are reported as usual.
   */
  #droppedThrough = 0n;

  /** @param core The core whose callbacks these are. Use {@link callbacks}. */
  constructor(core: UndraCore) {
    this.#core = core;
    // A core whose connection is lost (a remote one reconnecting, any one closed) holds none of them any more.
    core.connection.subscribe((state) => {
      if (state.kind === "reconnecting" || state.kind === "closed") this.#drop();
    });
  }

  /** How many instances the core holds references to. */
  get liveCount(): number {
    return this.#lent.size;
  }

  /** How many references the core holds to `impl` (0 when none). */
  count(impl: object): number {
    let count = 0;
    for (const lent of this.#lent.values()) if (lent.impl === impl) count += lent.count;
    return count;
  }

  /**
   * Lends `impl` to the core for one crossing: the instance handle to write (the same for the same object, counted once
   * more). The interface's bridge is registered with the core first, the first time. Generated code lends through
   * {@link lending}, which gives the reference back when the call never reached the core.
   */
  lend<T extends object>(impl: T, callback: CallbackInterface<T>): bigint {
    const spec = callback as unknown as CallbackInterface<object>;
    if (!this.#bridged.has(spec.portId)) {
      this.#core.registerPort(spec.portId, this.#bridge(spec));
      this.#bridged.add(spec.portId);
    }
    let instances = this.#instances.get(spec);
    if (instances === undefined) this.#instances.set(spec, (instances = new WeakMap()));
    let instance = instances.get(impl);
    const lent = instance === undefined ? undefined : this.#lent.get(instance);
    if (instance === undefined || lent === undefined) {
      instance = ++this.#next;
      instances.set(impl, instance);
      this.#lent.set(instance, { impl, callback: spec, count: 1 });
    } else {
      lent.count++;
    }
    return instance;
  }

  /** Takes back one reference the core never received (a refused call, or one that never reached it). */
  giveBack(instance: bigint): void {
    this.#unref(instance, "giveBack");
  }

  /** The core gave one reference back (`__release`); the entry goes when none is left. */
  release(instance: bigint): void {
    this.#unref(instance, "__release");
  }

  #unref(instance: bigint, why: string): void {
    const lent = this.#lent.get(instance);
    if (lent === undefined) {
      // Never early: an over-release is a bug (of a raw host, or of Undra), reported and otherwise ignored.
      if (instance > this.#droppedThrough) this.#core.report(new UndraError("state", `callback instance ${String(instance)} was released more often than it was lent`), why);
      return;
    }
    if (--lent.count > 0) return;
    this.#lent.delete(instance);
    this.#instances.get(lent.callback)?.delete(lent.impl);
  }

  /** Forgets every entry and aborts what runs: the core that held them is gone (or the connection to it). */
  #drop(): void {
    this.#droppedThrough = this.#next;
    this.#lent.clear();
    this.#instances.clear();
    for (const running of this.#running.values()) running.controller.abort();
    this.#running.clear();
    for (const pending of this.#coalesced.values()) pending.superseded = true;
    this.#coalesced.clear();
  }

  /** Runs `task` as the interface delivers: through the mirror's drain (`main`), or from a microtask (`background`). */
  #schedule(spec: CallbackInterface<object>, task: () => boolean): void {
    if (spec.background === true) queueMicrotask(task);
    else this.#core.mirror.enqueueCall(task);
  }

  /** The `PortImpl` that receives the core's calls into instances of `spec`. */
  #bridge(spec: CallbackInterface<object>): PortImpl {
    const methods: Record<number, (args: Uint8Array, portCallId?: number) => Promise<Uint8Array>> = {
      [spec.releaseInstance]: (args) => {
        const r = new UndraReader(args);
        const instance = r.readU64();
        r.finish();
        // After the invocations queued before it.
        this.#schedule(spec, () => {
          this.release(instance);
          return false;
        });
        return noReply();
      },
      [spec.cancelCall]: (args) => {
        const r = new UndraReader(args);
        r.readU64();
        const portCallId = r.readU32();
        r.finish();
        // At once: the running implementation sees its signal abort, an invocation that has not started is dropped.
        const running = this.#running.get(portCallId);
        if (running !== undefined) {
          this.#running.delete(portCallId);
          running.controller.abort();
        }
        return noReply();
      },
    };
    for (const [id, method] of Object.entries(spec.methods)) {
      methods[Number(id)] = (args, portCallId = 0) => this.#invoke(spec, Number(id), method, args, portCallId);
    }
    // A wasm core that restarts after a trap (ADR-049) disposes its ports: the instance that held these references is
    // gone, and the restored one holds none of them (a snapshot carries no callbacks).
    return { name: spec.name, sync: false, methods, dispose: () => this.#drop() };
  }

  /** A call of the core into an instance: decoded now, queued, answered when the implementation is done. */
  #invoke(
    spec: CallbackInterface<object>,
    methodId: number,
    method: CallbackMethod<object>,
    args: Uint8Array,
    portCallId: number,
  ): Promise<Uint8Array> {
    const operation = `${spec.name}.${method.name}`;
    const unavailable = (): Promise<Uint8Array> => (portCallId === 0 ? noReply() : Promise.reject(QUIETLY_UNAVAILABLE));
    let instance: bigint;
    let run: CallbackNotify<object> | CallbackCall<object>;
    try {
      const r = new UndraReader(args);
      instance = r.readU64();
      run = "notify" in method ? method.notify(r) : method.call(r);
      r.finish();
    } catch (error) {
      this.#core.report(error, operation);
      return unavailable();
    }
    // An instance the core no longer holds (or that a disconnect dropped): there is nobody to call.
    const lent = this.#lent.get(instance);
    if (lent === undefined) return unavailable();
    const impl = lent.impl;
    if ("notify" in method) {
      let pending: Pending | undefined;
      const key = `${String(instance)}:${methodId}`;
      if (method.coalesce === true) {
        const older = this.#coalesced.get(key);
        if (older !== undefined) older.superseded = true;
        this.#coalesced.set(key, (pending = { superseded: false }));
      }
      this.#schedule(spec, () => {
        if (pending !== undefined) {
          if (pending.superseded) return false;
          this.#coalesced.delete(key);
        }
        // Queued by a core that is gone since (a drop between the call and the drain): nobody holds the instance.
        if (this.#lent.get(instance) !== lent) return false;
        try {
          const result = (run as CallbackNotify<object>)(impl);
          if (result instanceof Promise) {
            result.catch((error: unknown) => {
              this.#core.report(error, operation);
            });
          }
        } catch (error) {
          this.#core.report(error, operation);
        }
        return true;
      });
      return portCallId === 0 ? noReply() : Promise.resolve(new Uint8Array(0));
    }
    const running: Running = { controller: new AbortController() };
    if (portCallId !== 0) this.#running.set(portCallId, running);
    return new Promise<Uint8Array>((resolve, reject) => {
      this.#schedule(spec, () => {
        if (portCallId !== 0 && this.#running.get(portCallId) !== running) {
          // Cancelled (or dropped) before it started.
          reject(QUIETLY_UNAVAILABLE);
          return false;
        }
        const signal = running.controller.signal;
        new Promise<Uint8Array>((settle) => {
          settle((run as CallbackCall<object>)(impl, signal));
        }).then(
          (body) => {
            if (this.#running.get(portCallId) === running) this.#running.delete(portCallId);
            resolve(body);
          },
          (error: unknown) => {
            if (this.#running.get(portCallId) === running) this.#running.delete(portCallId);
            if (error instanceof UndraPortError) {
              reject(error);
              return;
            }
            // A cancelled call's own abort, and a weak wrapper whose target is gone, are not failures of the app.
            if (!signal.aborted && error !== GONE) this.#core.report(error, operation);
            reject(QUIETLY_UNAVAILABLE);
          },
        );
        return true;
      });
    });
  }
}

const registries = new WeakMap<UndraCore, UndraCallbacks>();

/** The host callbacks of `core` (ADR-041): what the core holds, for statistics and tests. */
export function callbacks(core: UndraCore): UndraCallbacks {
  let registry = registries.get(core);
  if (registry === undefined) registries.set(core, (registry = new UndraCallbacks(core)));
  return registry;
}

/** Lends `impl` to `core` for one crossing and returns its instance handle; see {@link UndraCallbacks.lend}. */
export function lend<T extends object>(core: UndraCore, impl: T, callback: CallbackInterface<T>): bigint {
  return callbacks(core).lend(impl, callback);
}

/** Takes back one reference `core` never received; see {@link UndraCallbacks.giveBack}. */
export function giveBack(core: UndraCore, instance: bigint): void {
  callbacks(core).giveBack(instance);
}

/** Whether `error` is the reason `signal` aborted with. */
function abortedBy(signal: AbortSignal | undefined, error: unknown): boolean {
  return signal !== undefined && signal.aborted && error === signal.reason;
}

/**
 * Sends a call whose arguments lend host callbacks: `send` encodes the arguments, lending each callback through the
 * function it is given, and starts the call. A call the core refused (reply status 5) or that never reached it (the
 * encoding failed, the core is closed or unreachable, `signal` had already aborted) transfers nothing, so the lent
 * references are given back; any other outcome leaves them with the core, which gives them back with `__release`.
 */
export async function lending<R>(core: UndraCore, send: (lend: Lend) => Promise<R>, signal?: AbortSignal): Promise<R> {
  const lent: bigint[] = [];
  const unsent = signal?.aborted === true;
  try {
    return await send((impl, callback) => {
      const instance = lend(core, impl, callback);
      lent.push(instance);
      return instance;
    });
  } catch (error) {
    // The core owns what reached it: a reply other than a refusal, or the caller's abort of a call that was sent.
    const reached = error instanceof UndraReplyError ? error.status !== ReplyStatus.BadRequest : abortedBy(signal, error);
    if (unsent || !reached) for (const instance of lent) giveBack(core, instance);
    throw error;
  }
}

/**
 * What a generated weak wrapper's `async` method answers once its target was collected: a rejection the bridge turns
 * into "unavailable" for the core, with nothing reported.
 */
export function callbackGone(): Promise<never> {
  return Promise.reject(GONE);
}
