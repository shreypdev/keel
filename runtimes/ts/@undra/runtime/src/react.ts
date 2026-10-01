import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import type { UndraCore } from "./core.js";
import { type UndraClass, isUndraClass, openUndra } from "./lifetime.js";
import type { UndraObject } from "./object.js";
import type { Signal } from "./signal.js";

/*
 * `@undra/runtime/react` (docs/SPEC.md section 10.3): two hooks. `useSignal` reads a core signal
 * the way `useSyncExternalStore` wants it; `useUndra` owns the life of a store or object for a
 * component. `react` is a peer dependency; nothing else in `@undra/runtime` imports it.
 */

export type { UndraClass } from "./lifetime.js";

const noop = (): void => {};

/**
 * The current value of a core signal; the component renders again when it changes.
 *
 * Reading is local: the core pushes every change into the signal through the mirror, so a render
 * never calls into the core. Built on `useSyncExternalStore`, so it does not tear in concurrent
 * rendering, and it works for server rendering: on the server (and while hydrating) it reads the
 * signal's value as it is, which for a store nothing has observed is its initial value.
 *
 * `null` or `undefined` give `undefined`, so the hook can be called before a store exists:
 *
 * ```tsx
 * const todos = useUndra(Todos);
 * const visible = useSignal(todos?.visible); // Todo[] | undefined
 * ```
 */
export function useSignal<T>(signal: Signal<T>): T;
export function useSignal<T>(signal: Signal<T> | null | undefined): T | undefined;
export function useSignal<T>(signal: Signal<T> | null | undefined): T | undefined {
  // Stable per signal: a new `subscribe` function makes React unsubscribe and subscribe again.
  const subscribe = useCallback(
    (onChange: () => void): (() => void) => (signal === null || signal === undefined ? noop : signal.subscribe(() => onChange())),
    [signal],
  );
  // `peek()` returns the same reference until the signal changes, as `getSnapshot` requires.
  const getSnapshot = useCallback((): T | undefined => signal?.peek(), [signal]);
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/** What `useUndra` holds: the object, or why creating it failed, for the inputs it was created for. */
interface Held<S> {
  readonly inputs: readonly unknown[];
  readonly outcome: { readonly object: S } | { readonly error: unknown };
}

function sameInputs(a: readonly unknown[], b: readonly unknown[]): boolean {
  return a.length === b.length && a.every((value, index) => Object.is(value, b[index]));
}

/**
 * Creates a store or object when the component mounts and closes it when the component unmounts.
 *
 * ```tsx
 * function Counter() {
 *   const counter = useUndra(Counter);          // Counter | undefined: undefined until it exists
 *   const count = useSignal(counter?.count);
 *   if (counter === undefined) return null;
 *   return <button onClick={() => void counter.increment()}>{count}</button>;
 * }
 * ```
 *
 * The object is created in an effect, so it is `undefined` on the server, on the first render and
 * until the core has answered. If creating it fails, the error is thrown during render, where an
 * error boundary catches it. The component owns the object: two components that need the same state
 * should take one store as a prop (or from context) instead of each creating their own. In
 * `StrictMode` development builds, React mounts components twice, which creates the object twice and
 * closes the first.
 *
 * @param type A generated class (`Todos`). Its `create(core)` is called with `core`, default `UndraCore.shared`.
 * @param core The core to create the object in.
 */
export function useUndra<S extends UndraObject>(type: UndraClass<S>, core?: UndraCore): S | undefined;
/**
 * The same for an object whose `create` takes arguments (a query handle, for instance): `create`
 * is called again, and the previous object closed, whenever one of `deps` changes.
 *
 * ```tsx
 * const page = useUndra(() => TodosQueryHandle.create(pageNumber), [pageNumber]);
 * ```
 */
export function useUndra<S extends UndraObject>(create: () => Promise<S>, deps: readonly unknown[]): S | undefined;
export function useUndra<S extends UndraObject>(
  source: UndraClass<S> | (() => Promise<S>),
  second?: UndraCore | readonly unknown[],
): S | undefined {
  const [held, setHeld] = useState<Held<S> | undefined>(undefined);
  const inputs: readonly unknown[] = isUndraClass(source) ? [source, second] : ((second as readonly unknown[] | undefined) ?? []);
  // A fresh closure each render; the effect below only reads it when its inputs have changed.
  const create = isUndraClass(source) ? () => source.create(second as UndraCore | undefined) : source;
  useEffect(
    () =>
      openUndra(
        create,
        (object) => setHeld({ inputs, outcome: { object } }),
        (error) => setHeld({ inputs, outcome: { error } }),
      ),
    inputs,
  );
  // A result for other inputs belongs to an object that is being closed.
  if (held === undefined || !sameInputs(held.inputs, inputs)) return undefined;
  if ("error" in held.outcome) throw held.outcome.error;
  return held.outcome.object;
}
