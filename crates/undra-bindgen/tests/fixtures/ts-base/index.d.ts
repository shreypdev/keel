// Hand-written declaration of the `@undra/runtime` base API that generated TypeScript depends on.
//
// SPEC section 17.1 lists these names, but the runtime package does not implement them yet, so the
// type-check test (`tests/typecheck.rs`) pairs this file with the *real* wire-layer declarations,
// built from `runtimes/ts/@undra/runtime/src` on every run. Everything under "Additions" is API the
// generated code needs that section 17.1 does not spell out; the bindgen report lists it for the
// runtime authors.

export * from "./fnv.js";
export * from "./wire/index.js";
// The real standard types of SPEC section 8 (`HttpRequest`, `HttpError`, ..., and their codecs).
export * from "./adapters/types.js";
export * from "./adapters/codecs.js";
// The real errors: the base class, the reply and transport errors, and the closed set generated calls map onto
// (ADR-032, amendment A).
export * from "./errors.js";
export * from "./call-error.js";

import type { CallTarget, ChangeOp, Handle, UndraReader } from "./wire/index.js";

export type LoadMode = "wasm-main" | "wasm-worker" | "remote";

export interface LoadOptions {
  readonly mode: LoadMode;
  readonly wasm?: URL | BufferSource;
  readonly url?: string;
  readonly adapters?: Readonly<Record<string, PortImpl>>;
  readonly expectedSchemaHash: bigint;
  /** ADR-044 amendment A: the core's namespace, which the generated entry fills in (`UndraIds.namespace`). */
  readonly namespace?: string;
}

/** Addition (ADR-044): what `UndraCore.attach` and a generated entry's `attach` take. */
export interface AttachOptions {
  readonly adapters?: Readonly<Record<string, PortImpl>>;
  readonly expectedSchemaHash: bigint;
  readonly shared?: boolean;
  /** ADR-044 amendment A: the core's namespace, which the generated entry fills in (`UndraIds.namespace`). */
  readonly namespace?: string;
}

/** Addition (ADR-044): a transport a host provides (React Native's `NativeTransport`, a test double). */
export interface Transport {
  readonly mode: string;
}

export interface UndraStats {
  readonly liveHandles: number;
}

/**
 * Addition: what a call addresses. The wire `CallTarget` enum carries only the discriminant, but a
 * method call also needs the handle it runs on, so generated code passes this object instead.
 */
export type CallTargetRef =
  | { readonly target: CallTarget.FreeFunction }
  | { readonly target: CallTarget.ObjectMethod; readonly handle: Handle };

export interface PortImpl {
  /** Addition (ADR-049): the port's name, for the runtime's messages. */
  readonly name?: string;
  readonly sync: boolean;
  readonly methods: Readonly<Record<number, (args: Uint8Array) => Uint8Array | Promise<Uint8Array>>>;
}

export interface Mirror {
  /** Addition (ADR-031): `options.noCoalesce` lists the store's `no_coalesce` signals. */
  register(
    handle: Handle,
    apply: (signalId: number, op: ChangeOp, value: Uint8Array) => void,
    options?: { readonly noCoalesce?: Iterable<number> },
  ): void;
  unregister(handle: Handle): void;
}

/** Addition (ADR-031): what a generated store tells its base class (its `no_coalesce` signals). */
export interface StoreOptions {
  readonly noCoalesce?: readonly number[];
  /** Addition (ADR-049): the recorded constructor call of a query handle, which the runtime re-creates after a restart. */
  readonly recreate?: { readonly typeId: number; readonly methodId: number; readonly args: Uint8Array };
}

export declare class UndraCore {
  static load(options: LoadOptions): Promise<UndraCore>;
  static get shared(): UndraCore;
  /** Addition (ADR-032, amendment A): the loaded shared core, or `null`. */
  static get current(): UndraCore | null;
  /** Addition (ADR-044): runs a core over a transport the caller provides. */
  static attach(transport: Transport, options: AttachOptions): Promise<UndraCore>;
  /** Addition (ADR-044): the closed placeholder a generated entry's `core` is while its core is not loaded. */
  static get unloaded(): UndraCore;
  /** Whether the core was closed. */
  readonly closed: boolean;
  callSync(target: CallTargetRef, methodId: number, args: Uint8Array): Uint8Array;
  call(target: CallTargetRef, methodId: number, args: Uint8Array, signal?: AbortSignal): Promise<Uint8Array>;
  stream(target: CallTargetRef, methodId: number, args: Uint8Array): AsyncIterable<Uint8Array>;
  construct(typeId: number, methodId: number, args: Uint8Array): Promise<Handle>;
  /**
   * Addition: resolves once the initial change-set of the observed signals has been applied to the
   * mirror (immediately for in-process cores), so a store never exposes placeholder values.
   */
  observe(handle: Handle, signalId: number, on: boolean): Promise<void>;
  release(handle: Handle): void;
  /** Addition: sends a host-to-core event of an event port (`undra_event`). */
  event(portId: number, methodId: number, payload: Uint8Array): void;
  /** Addition (ADR-032, amendment A): reports a failure no caller can see (a command, a store's `_apply`). */
  report(error: unknown, operation: string): void;
  readonly mirror: Mirror;
  registerPort(portId: number, impl: PortImpl): void;
  stats(): Promise<UndraStats>;
}

export declare abstract class UndraObject {
  protected constructor(core: UndraCore, handle: Handle);
  readonly core: UndraCore;
  readonly handle: Handle;
  close(): void;
}

export declare abstract class UndraStore extends UndraObject {
  protected constructor(core: UndraCore, handle: Handle, options?: StoreOptions);
  protected _signals: Signal<unknown>[];
  protected abstract _apply(signalId: number, op: ChangeOp, value: Uint8Array): void;
  /** Addition (ADR-032, amendment A): observes every signal; on failure closes the store and throws an `UndraCallError`. */
  protected _observeAll(): Promise<void>;
}

export declare class Signal<T> {
  /** Addition: the constructor, taking the placeholder value shown until the first change-set. */
  constructor(initial: T);
  get(): T;
  peek(): T;
  subscribe(fn: (value: T) => void): () => void;
  _set(value: T): void;
}

export interface UndraPort {}

/** Addition (ADR-040): a generated class as `adopt` takes it. */
export interface UndraObjectClass<T extends UndraObject> {
  readonly prototype: T;
}

/** Addition (ADR-040): the wrapper of a handle the core handed over, one per handle. */
export declare function adopt<T extends UndraObject>(core: UndraCore, handle: Handle, type: UndraObjectClass<T>): T;
/** Addition (ADR-040): the object a reply carries, adopted (a new store is observed first). */
export declare function adoptObject<T extends UndraObject>(core: UndraCore, body: Uint8Array, type: UndraObjectClass<T>): Promise<T>;
/** Addition (ADR-040): the optional object a reply carries. */
export declare function adoptOptional<T extends UndraObject>(core: UndraCore, body: Uint8Array, type: UndraObjectClass<T>): Promise<T | null>;
/** Addition (ADR-040): the objects a reply carries. */
export declare function adoptList<T extends UndraObject>(core: UndraCore, body: Uint8Array, type: UndraObjectClass<T>): Promise<T[]>;
/** Addition (ADR-040): the handle of an object parameter; refuses an object of another core. */
export declare function requireOwn(core: UndraCore, object: UndraObject): Handle;

/** Addition (ADR-041): one method of a callback interface, as its generated bridge describes it. */
export type CallbackMethod<T> =
  | { readonly name: string; readonly coalesce?: boolean; readonly notify: (args: UndraReader) => (impl: T) => unknown }
  | { readonly name: string; readonly call: (args: UndraReader) => (impl: T, signal: AbortSignal) => Promise<Uint8Array> };

/** Addition (ADR-041): a host callback interface, as its generated bridge describes it. */
export interface CallbackInterface<T extends object> {
  readonly name: string;
  readonly portId: number;
  readonly releaseInstance: number;
  readonly cancelCall: number;
  readonly background?: boolean;
  readonly methods: Readonly<Record<number, CallbackMethod<T>>>;
}

/** Addition (ADR-041): lends a callback for the call being encoded. */
export type Lend = <T extends object>(impl: T, callback: CallbackInterface<T>) => bigint;
/** Addition (ADR-041): sends a call whose arguments lend callbacks; a refused or unsent call gives them back. */
export declare function lending<R>(core: UndraCore, send: (lend: Lend) => Promise<R>, signal?: AbortSignal): Promise<R>;
/** Addition (ADR-041): `lending` for a stream: every iteration lends again, and a refused or unopened stream gives back. */
export declare function lendingStream(core: UndraCore, open: (lend: Lend) => AsyncIterable<Uint8Array>): AsyncIterable<Uint8Array>;
/** Addition (ADR-041): lends a callback to a core outside `lending` (a stream's arguments). */
export declare function lend<T extends object>(core: UndraCore, impl: T, callback: CallbackInterface<T>): bigint;
/** Addition (ADR-041): what a weak wrapper's async method answers once its target is gone. */
export declare function callbackGone(): Promise<never>;
