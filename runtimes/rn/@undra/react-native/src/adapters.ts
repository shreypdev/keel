import { type AdapterOverrides, type AppState as UndraAppState, type LifecycleAdapter, PortIds } from "@undra/runtime";
import { AppState } from "react-native";
import { reactNativeHttp } from "./http.js";
import type { NativePlatformDefaults } from "./native.js";
import type { NativeLoadOptions } from "./load.js";

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
 * microtask, then every change; a state equal to the last one reported is not reported again
 * (React Native announces `active` up to three times while an app starts).
 */
export function appStateLifecycle(): LifecycleAdapter {
  return {
    subscribe(emit) {
      let active = true;
      let last: UndraAppState | null = null;
      const report = (state: string | null | undefined): void => {
        const next = lifecycleState(state);
        if (!active || next === last) return;
        last = next;
        emit(next);
      };
      queueMicrotask(() => {
        report(AppState.currentState);
      });
      const subscription = AppState.addEventListener("change", (state) => {
        report(state);
      });
      return () => {
        active = false;
        subscription.remove();
      };
    },
  };
}

/**
 * The JavaScript adapters `loadNative` adds to the TypeScript runtime's defaults (ADR-038 amendment B): `Http` over
 * React Native's `fetch` ({@link reactNativeHttp}) and `Lifecycle` from `AppState`. The other standard ports are the
 * module's own, natively: `Kv`, `SecureStore`, `Fs` and the `Connectivity` source (see {@link nativeDefaultPorts}),
 * and `Clock`, `Rng`, `Log` and `Timer` (decision 7 of ADR-038).
 */
export function reactNativeAdapters(): AdapterOverrides {
  return { http: reactNativeHttp(), lifecycle: appStateLifecycle() };
}

/** The adapter name (in `AttachOptions.adapters`) of each standard port the module can answer natively. */
const NATIVE_DEFAULTS: ReadonlyArray<readonly [number, keyof AdapterOverrides]> = [
  [PortIds.Kv.portId, "kv"],
  [PortIds.SecureStore.portId, "secureStore"],
  [PortIds.Fs.portId, "fs"],
  [PortIds.Connectivity.portId, "connectivity"],
];

/**
 * Which standard ports the module answers natively for these options (ADR-038 amendment B, B7): those the platform
 * offers (`platformDefaults().ports`) that the app did not override. A port is overridden when `adapters` has a value
 * for it, an adapter (JavaScript) or `null` (no adapter: the port is unavailable), or when `ports` has an
 * implementation for its id.
 */
export function nativeDefaultPorts(
  offered: Pick<NativePlatformDefaults, "ports">,
  options: Pick<NativeLoadOptions, "adapters" | "ports">,
): number[] {
  const chosen: number[] = [];
  for (const [portId, name] of NATIVE_DEFAULTS) {
    if (!offered.ports.includes(portId)) continue;
    if (options.adapters?.[name] !== undefined) continue;
    if (options.ports?.[portId] !== undefined) continue;
    chosen.push(portId);
  }
  return chosen;
}
