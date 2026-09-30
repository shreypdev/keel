import { type MaybeRefOrGetter, type ShallowRef, getCurrentScope, onScopeDispose, shallowRef, toRaw, toValue, watch } from "vue";
import type { KeelCore } from "./core.js";
import { type KeelClass, openKeel, rethrowLater } from "./lifetime.js";
import type { KeelObject } from "./object.js";
import type { Signal } from "./signal.js";

/*
 * `@keel/runtime/vue` (docs/SPEC.md section 10.3): a signal as a ref, and a store whose life is the
 * scope it is created in. `vue` (3.3 or later) is a peer dependency.
 */

export type { KeelClass } from "./lifetime.js";

/**
 * A core signal as a read-only ref. The ref holds the value itself, not a reactive copy of it
 * (`shallowRef`), so a 10,000-row list is not wrapped in proxies, and it is replaced, not mutated,
 * when the core changes the signal.
 *
 * The argument may be a signal, a ref or a getter of one, or `null`/`undefined` (the ref is then
 * `undefined`), so it can follow a store that does not exist yet:
 *
 * ```ts
 * const todos = useKeel(Todos);
 * const visible = useSignal(() => todos.value?.visible); // Readonly<ShallowRef<Todo[] | undefined>>
 * ```
 *
 * The subscription ends with the component (or effect scope) the call is made in.
 */
export function useSignal<T>(signal: MaybeRefOrGetter<Signal<T>>): Readonly<ShallowRef<T>>;
export function useSignal<T>(signal: MaybeRefOrGetter<Signal<T> | null | undefined>): Readonly<ShallowRef<T | undefined>>;
export function useSignal<T>(signal: MaybeRefOrGetter<Signal<T> | null | undefined>): Readonly<ShallowRef<T | undefined>> {
  const state = shallowRef<T | undefined>(undefined);
  watch(
    // `toRaw`: a signal kept in a deep `ref()` comes back as a proxy, and a proxy cannot reach the private fields of a `Signal`.
    () => toRaw(toValue(signal)),
    (current, _previous, onCleanup) => {
      state.value = current?.peek();
      if (current !== null && current !== undefined) {
        onCleanup(
          current.subscribe((value) => {
            state.value = value;
          }),
        );
      }
    },
    // Immediately, so the first read is the current value; synchronously, so a change is visible to the code that follows it.
    { immediate: true, flush: "sync" },
  );
  return state;
}

/**
 * Creates a store or object now and closes it when the scope the call is made in (a component's
 * `setup`) ends. The ref is `undefined` until the core has answered. If creating it fails the error
 * is rethrown from a microtask, as an uncaught error.
 *
 * ```ts
 * const counter = useKeel(Counter);
 * const count = useSignal(() => counter.value?.count);
 * ```
 *
 * @param type A generated class (`Counter`). Its `create(core)` is called with `core`, default `KeelCore.shared`.
 */
export function useKeel<S extends KeelObject>(type: KeelClass<S>, core?: KeelCore): Readonly<ShallowRef<S | undefined>> {
  // The cast: `shallowRef`'s overloads cannot be resolved for a type parameter.
  const store = shallowRef<S | undefined>(undefined) as ShallowRef<S | undefined>;
  const close = openKeel(
    () => type.create(core),
    (created) => {
      store.value = created;
    },
    rethrowLater,
  );
  if (getCurrentScope() !== undefined) onScopeDispose(close);
  return store;
}
