import type { KeelCore } from "./core.js";
import type { KeelObject } from "./object.js";

/**
 * A generated object or store class as the framework adapters take it
 * (`Todos`, `Counter`, ...): anything with the `static create(core?)` that
 * `keel-bindgen` emits.
 *
 * ```ts
 * const todos = useKeel(Todos); // `typeof Todos` is a KeelClass<Todos>
 * ```
 */
export interface KeelClass<S extends KeelObject> {
  /** Creates the object in `core` (default `KeelCore.shared`). */
  create(core?: KeelCore): Promise<S>;
}

/** Whether `source` is a generated class (it has a static `create`) rather than a plain factory. */
export function isKeelClass<S extends KeelObject>(source: KeelClass<S> | (() => Promise<S>)): source is KeelClass<S> {
  return typeof (source as Partial<KeelClass<S>>).create === "function";
}

/**
 * The life of one object for a framework adapter: creates it, hands it to
 * `onReady`, and returns the function that ends it. Ending closes the object
 * now, or as soon as its creation finishes, and silences both callbacks, so
 * an effect that re-runs or a component that unmounts while `create` is still
 * pending neither leaks a handle nor reports to something that is gone.
 *
 * @internal Used by the adapters in `react`, `vue` and `solid`.
 */
export function openKeel<S extends KeelObject>(
  create: () => Promise<S>,
  onReady: (object: S) => void,
  onError: (error: unknown) => void,
): () => void {
  let ended = false;
  let object: S | undefined;
  // `new Promise` so that a factory that throws before it returns a promise is a failed creation too.
  new Promise<S>((resolve) => {
    resolve(create());
  }).then(
    (created) => {
      if (ended) {
        created.close();
        return;
      }
      object = created;
      onReady(created);
    },
    (error: unknown) => {
      if (!ended) onError(error);
    },
  );
  return () => {
    ended = true;
    object?.close();
    object = undefined;
  };
}

/** Rethrows `error` from a microtask, so it surfaces as an uncaught error where nothing can catch it. */
export function rethrowLater(error: unknown): void {
  queueMicrotask(() => {
    throw error;
  });
}
