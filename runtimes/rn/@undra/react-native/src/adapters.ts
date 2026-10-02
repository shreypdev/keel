import {
  type AdapterOverrides,
  type HttpAdapter,
  type LifecycleAdapter,
  PortIds,
  type PortImpl,
  type AppState as UndraAppState,
} from "@undra/runtime";
import { OptInPortIds, type SseAdapter, type WebSocketAdapter, ssePort, webSocketPort } from "@undra/runtime/realtime";
import { AppState } from "react-native";
import { reactNativeHttp } from "./http.js";
import type { NativePlatformDefaults } from "./native.js";
import type { NativeLoadOptions } from "./load.js";
import { reactNativeSse, reactNativeWebSocket } from "./realtime.js";

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

/** The JavaScript defaults of `loadNative` (see {@link reactNativeAdapters}). */
export interface ReactNativeAdapters {
  /** `Http` over React Native's `fetch` ({@link reactNativeHttp}). */
  readonly http: HttpAdapter;
  /** `Lifecycle` from `AppState` ({@link appStateLifecycle}). */
  readonly lifecycle: LifecycleAdapter;
  /** The opt-in `WebSocket` port's adapter (ADR-047): React Native's `WebSocket` ({@link reactNativeWebSocket}). */
  readonly webSocket: WebSocketAdapter;
  /** The opt-in `Sse` port's adapter (ADR-047): a streaming `fetch`, else `XMLHttpRequest` ({@link reactNativeSse}). */
  readonly sse: SseAdapter;
}

/**
 * The JavaScript adapters `loadNative` adds to the TypeScript runtime's defaults (ADR-038 amendment B; ADR-047):
 * `Http` over React Native's `fetch` ({@link reactNativeHttp}), `Lifecycle` from `AppState`, and the adapters of the
 * opt-in `WebSocket` and `Sse` ports, which `loadNative` registers through `@undra/runtime/realtime`'s bindings
 * ({@link realtimePorts}). The other standard ports are the module's own, natively: `Kv`, `SecureStore`, `Fs`, `Db`
 * (ADR-048) and the `Connectivity` source (see {@link nativeDefaultPorts}), and `Clock`, `Rng`, `Log` and `Timer`
 * (decision 7 of ADR-038).
 */
export function reactNativeAdapters(): ReactNativeAdapters {
  return { http: reactNativeHttp(), lifecycle: appStateLifecycle(), webSocket: reactNativeWebSocket(), sse: reactNativeSse() };
}

/**
 * The `WebSocket` and `Sse` ports over `adapters` (ADR-047), by port id, for `AttachOptions.ports`: fresh bindings
 * (one per core: each owns its connections and closes them with the core).
 */
export function realtimePorts(adapters: Pick<ReactNativeAdapters, "webSocket" | "sse">): Record<number, PortImpl> {
  return {
    [OptInPortIds.WebSocket.portId]: webSocketPort(adapters.webSocket),
    [OptInPortIds.Sse.portId]: ssePort(adapters.sse),
  };
}

/**
 * Each standard port the module can answer natively, with its adapter name in `AttachOptions.adapters` (`null` for
 * `Db`, which has none: an implementation in `ports` replaces it).
 */
const NATIVE_DEFAULTS: ReadonlyArray<readonly [number, keyof AdapterOverrides | null]> = [
  [PortIds.Kv.portId, "kv"],
  [PortIds.SecureStore.portId, "secureStore"],
  [PortIds.Fs.portId, "fs"],
  [PortIds.Connectivity.portId, "connectivity"],
  [OptInPortIds.Db.portId, null],
];

/**
 * Which standard ports the module answers natively for these options (ADR-038 amendment B, B7; ADR-048): those the
 * platform offers (`platformDefaults().ports`) that the app did not override. A port is overridden when `adapters`
 * has a value for it, an adapter (JavaScript) or `null` (no adapter: the port is unavailable), or when `ports` has an
 * implementation for its id (the only way to replace `Db`).
 */
export function nativeDefaultPorts(
  offered: Pick<NativePlatformDefaults, "ports">,
  options: Pick<NativeLoadOptions, "adapters" | "ports">,
): number[] {
  const chosen: number[] = [];
  for (const [portId, name] of NATIVE_DEFAULTS) {
    if (!offered.ports.includes(portId)) continue;
    if (name !== null && options.adapters?.[name] !== undefined) continue;
    if (options.ports?.[portId] !== undefined) continue;
    chosen.push(portId);
  }
  return chosen;
}

/** The adapter names (in `AttachOptions.adapters`) of `portIds`, the standard ports the module answers natively. */
export function nativeAdapterNames(portIds: readonly number[]): Array<keyof AdapterOverrides> {
  const names: Array<keyof AdapterOverrides> = [];
  for (const [portId, name] of NATIVE_DEFAULTS) {
    if (name !== null && portIds.includes(portId)) names.push(name);
  }
  return names;
}
