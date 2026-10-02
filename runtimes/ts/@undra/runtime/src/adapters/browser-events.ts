import { consoleLog, setTimeoutTimer } from "./system.js";
import type { Adapters, AppState, ConnectivityAdapter, LifecycleAdapter, NetKind } from "./types.js";

/** The parts of a `Navigator` that {@link browserConnectivity} reads. */
export interface NavigatorLike {
  readonly onLine?: boolean;
  readonly connection?: { readonly type?: string; readonly effectiveType?: string };
}

/** Options of {@link browserConnectivity}. */
export interface BrowserConnectivityOptions {
  /** Where `online` and `offline` events fire; default `globalThis` (the window). */
  readonly target?: Pick<EventTarget, "addEventListener" | "removeEventListener">;
  /** Default the global `navigator`. */
  readonly navigator?: NavigatorLike;
}

function netKind(nav: NavigatorLike, online: boolean): NetKind {
  if (!online) return "none";
  switch (nav.connection?.type) {
    case "wifi":
      return "wifi";
    case "cellular":
      return "cellular";
    case "ethernet":
      return "wired";
    case "none":
      return "none";
    default:
      return "unknown";
  }
}

/**
 * The `Connectivity` event source of browsers: `navigator.onLine` with the
 * `online` and `offline` events. The connection type comes from the Network
 * Information API where it exists (Chromium) and is `"unknown"` elsewhere.
 * `subscribe` reports the current state from a microtask, then every change.
 */
export function browserConnectivity(options: BrowserConnectivityOptions = {}): ConnectivityAdapter {
  return {
    subscribe(emit) {
      const nav = options.navigator ?? (globalThis as { navigator?: NavigatorLike }).navigator ?? {};
      const target = options.target ?? (globalThis as unknown as Pick<EventTarget, "addEventListener" | "removeEventListener">);
      let active = true;
      const report = (): void => {
        if (!active) return;
        const online = nav.onLine !== false;
        emit(online, netKind(nav, online));
      };
      target.addEventListener("online", report);
      target.addEventListener("offline", report);
      queueMicrotask(report);
      return () => {
        active = false;
        target.removeEventListener("online", report);
        target.removeEventListener("offline", report);
      };
    },
  };
}

/** The parts of a `Document` that {@link browserLifecycle} reads. */
export interface DocumentLike extends Pick<EventTarget, "addEventListener" | "removeEventListener"> {
  readonly visibilityState?: string;
}

/**
 * The `Lifecycle` event source of browsers: the Page Visibility API. A visible
 * page is `active`, a hidden one (another tab, a minimised window, a locked
 * phone) `background`; browsers have no `inactive` state. `subscribe` reports
 * the current state from a microtask, then every change.
 */
export function browserLifecycle(document?: DocumentLike): LifecycleAdapter {
  return {
    subscribe(emit) {
      const doc = document ?? (globalThis as { document?: DocumentLike }).document;
      if (doc === undefined) return () => {};
      let active = true;
      const report = (): void => {
        if (!active) return;
        const state: AppState = doc.visibilityState === "hidden" ? "background" : "active";
        emit(state);
      };
      doc.addEventListener("visibilitychange", report);
      queueMicrotask(report);
      return () => {
        active = false;
        doc.removeEventListener("visibilitychange", report);
      };
    },
  };
}

/**
 * The adapters a page has without loading anything: timers over `setTimeout`, the console, and the
 * `Connectivity` and `Lifecycle` sources where the platform has them. `UndraCore` starts from these;
 * the four port adapters of {@link browserAdapters} (Http, Kv, SecureStore, Fs) are built, with the
 * code of their ports, on the first call to the port (ADR-052).
 */
export function lightAdapters(): Partial<Adapters> {
  const g = globalThis as {
    navigator?: { onLine?: unknown };
    document?: { visibilityState?: unknown };
    addEventListener?: unknown;
  };
  const adapters: Partial<Adapters> = { timer: setTimeoutTimer(), log: consoleLog() };
  if (typeof g.navigator?.onLine === "boolean" && typeof g.addEventListener === "function") {
    adapters.connectivity = browserConnectivity();
  }
  if (g.document?.visibilityState !== undefined) adapters.lifecycle = browserLifecycle();
  return adapters;
}
