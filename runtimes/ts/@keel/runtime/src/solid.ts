import { type Accessor, createSignal, getOwner, onCleanup } from "solid-js";
import type { KeelCore } from "./core.js";
import { type KeelClass, openKeel, rethrowLater } from "./lifetime.js";
import type { KeelObject } from "./object.js";
import type { Signal } from "./signal.js";

/*
 * `@keel/runtime/solid` (docs/SPEC.md section 10.3): a core signal as a Solid accessor, and a store
 * whose life is the owner (component or root) it is created in. `solid-js` is a peer dependency.
 */

export type { KeelClass } from "./lifetime.js";

/**
 * A core signal as an accessor: read it in JSX, `createMemo` or `createEffect` and Solid tracks it.
 * The subscription ends with the owner the call is made in.
 *
 * ```tsx
 * const visible = useSignal(todos.visible);
 * return <For each={visible()}>{(todo) => <li>{todo.title}</li>}</For>;
 * ```
 */
export function useSignal<T>(signal: Signal<T>): Accessor<T> {
  // `() => next`: Solid would call a value that is itself a function.
  const [value, setValue] = createSignal<T>(signal.peek());
  const stop = signal.subscribe((next) => setValue(() => next));
  if (getOwner() !== null) onCleanup(stop);
  return value;
}

/**
 * Creates a store or object now and closes it when the owner the call is made in ends. The accessor
 * is `undefined` until the core has answered, so a component that needs the store renders it under
 * `<Show when={counter()}>`. If creating it fails the error is rethrown from a microtask, as an
 * uncaught error.
 *
 * ```tsx
 * const counter = useKeel(Counter);
 * return <Show when={counter()}>{(store) => <CounterView counter={store()} />}</Show>;
 * ```
 *
 * @param type A generated class (`Counter`). Its `create(core)` is called with `core`, default `KeelCore.shared`.
 */
export function useKeel<S extends KeelObject>(type: KeelClass<S>, core?: KeelCore): Accessor<S | undefined> {
  const [store, setStore] = createSignal<S | undefined>(undefined);
  const close = openKeel(
    () => type.create(core),
    (created) => setStore(() => created),
    rethrowLater,
  );
  if (getOwner() !== null) onCleanup(close);
  return store;
}
