import type { Signal } from "@keel/runtime";
import { useSyncExternalStore } from "react";

/**
 * The current value of a core signal; the component renders again when the core changes it.
 *
 * Reading is local: the core pushes every change through the runtime's mirror, so this never
 * calls into the core. `peek` returns the same reference until the signal changes, which is
 * what `useSyncExternalStore` needs.
 */
export function useSignal<T>(signal: Signal<T>): T {
  return useSyncExternalStore(
    (onChange) => signal.subscribe(() => onChange()),
    () => signal.peek(),
  );
}
