import type { Signal } from "@keel/runtime";
import { useSyncExternalStore } from "react";

/** The current value of a core signal; the component renders again when the core changes it. */
export function useSignal<T>(signal: Signal<T>): T {
  return useSyncExternalStore(
    (onChange) => signal.subscribe(() => onChange()),
    () => signal.peek(),
  );
}
