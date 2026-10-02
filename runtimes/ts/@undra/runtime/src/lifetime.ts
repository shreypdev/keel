import type { UndraCore } from "./core.js";
import type { UndraObject } from "./object.js";

/**
 * A generated object or store class as the framework adapters take it
 * (`Todos`, `Counter`, ...): anything with the `static create(core?)` that
 * `undra-bindgen` emits.
 *
 * ```ts
 * const todos = useUndra(Todos); // `typeof Todos` is an UndraClass<Todos>
 * ```
 */
export interface UndraClass<S extends UndraObject> {
  /** Creates the object in `core` (default `UndraCore.shared`). */
  create(core?: UndraCore): Promise<S>;
}

/** Whether `source` is a generated class (it has a static `create`) rather than a plain factory. */
export function isUndraClass<S extends UndraObject>(source: UndraClass<S> | (() => Promise<S>)): source is UndraClass<S> {
  return typeof (source as Partial<UndraClass<S>>).create === "function";
}

/**
 * How many owners (`openUndra` lives) hold each object. One wrapper per handle (ADR-040 decision 7) means two creations
 * that the core answers with one handle (a singleton constructor, `Arc<Self>`: StrictMode's second effect, or two
 * components that ask for the same one) reach two owners as the *same* wrapper; the first to end must not close it under
 * the other.
 */
const owners = new WeakMap<UndraObject, number>();

/**
 * The life of one object for a framework adapter: creates it, hands it to
 * `onReady`, and returns the function that ends it. Ending closes the object
 * now, or as soon as its creation finishes, and silences both callbacks, so
 * an effect that re-runs or a component that unmounts while `create` is still
 * pending neither leaks a handle nor reports to something that is gone. An object
 * that another life holds too (the same wrapper) is closed when the last one ends.
 *
 * @internal Used by the adapters in `react`, `vue` and `solid`.
 */
export function openUndra<S extends UndraObject>(
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
        // Its life ended before it arrived. A life that began meanwhile may be handed the same wrapper in this very
        // turn (StrictMode's second effect against a transport that answers inside the call), so whether anyone holds
        // it is looked at on the next one.
        setTimeout(() => {
          if ((owners.get(created) ?? 0) === 0) created.close();
        }, 0);
        return;
      }
      object = created;
      owners.set(created, (owners.get(created) ?? 0) + 1);
      onReady(created);
    },
    (error: unknown) => {
      if (!ended) onError(error);
    },
  );
  return () => {
    ended = true;
    const held = object;
    object = undefined;
    if (held === undefined) return;
    const left = (owners.get(held) ?? 1) - 1;
    if (left > 0) {
      owners.set(held, left);
      return;
    }
    owners.delete(held);
    held.close();
  };
}

/** Rethrows `error` from a microtask, so it surfaces as an uncaught error where nothing can catch it. */
export function rethrowLater(error: unknown): void {
  queueMicrotask(() => {
    throw error;
  });
}
