import type { AdapterOverrides, AppState as UndraAppState, LifecycleAdapter } from "@undra/runtime";
import { AppState } from "react-native";

/** React Native's `AppState` (`"active"`, `"inactive"`, `"background"`, ...) as the core's `Lifecycle` state. */
export function lifecycleState(state: string | null | undefined): UndraAppState {
  switch (state) {
    case "active":
      return "active";
    case "inactive":
      return "inactive";
    default:
      return "background";
  }
}

/**
 * The `Lifecycle` event source of React Native: `AppState`. Reports the current state from a
 * microtask, then every change.
 */
export function appStateLifecycle(): LifecycleAdapter {
  return {
    subscribe(emit) {
      let active = true;
      queueMicrotask(() => {
        if (active) emit(lifecycleState(AppState.currentState));
      });
      const subscription = AppState.addEventListener("change", (state) => {
        if (active) emit(lifecycleState(state));
      });
      return () => {
        active = false;
        subscription.remove();
      };
    },
  };
}

/**
 * The adapters `loadNative` adds to the TypeScript runtime's defaults (which give `fetch` for
 * `Http`, the console for `Log` and `setTimeout` for `Timer` in React Native): `Lifecycle` from
 * `AppState`. React Native's core has no key-value store, file system or connectivity API, so `Kv`,
 * `SecureStore`, `Fs` and `Connectivity` are the app's to supply (docs/REACT_NATIVE.md). `Clock`,
 * `Rng` and `Timer` are native and need no adapter.
 */
export function reactNativeAdapters(): AdapterOverrides {
  return { lifecycle: appStateLifecycle() };
}
