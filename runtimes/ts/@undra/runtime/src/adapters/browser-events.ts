import { consoleLog, setTimeoutTimer } from "./system.js";
import type { Adapters, ConnectivityAdapter, LifecycleAdapter, NetKind } from "./types.js";

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
  /** The window the document is in: where `pagehide` fires. */
  readonly defaultView?: Pick<EventTarget, "addEventListener" | "removeEventListener"> | null;
}

/**
 * The `Lifecycle` event source of browsers: the Page Visibility API and the page lifecycle events. A visible page is
 * `active`; a hidden one (another tab, a minimised window, a locked phone) is `background`, and so is a page that is
 * being left or frozen (`pagehide`, `freeze`: ADR-046 decision 3.4, the last moment a page can still drain its
 * background work); browsers have no `inactive` state. `subscribe` reports the current state from a microtask, then every
 * change. `target` is where `pagehide` fires (default: the window of the document, `document.defaultView`).
 */
export function browserLifecycle(document?: DocumentLike, target?: Pick<EventTarget, "addEventListener" | "removeEventListener">): LifecycleAdapter {
  return {
    subscribe(emit) {
      const doc = document ?? (globalThis as { document?: DocumentLike }).document;
      if (doc === undefined) return () => {};
      const win = target ?? doc.defaultView ?? doc;
      let active = true;
      const report = (event?: Event): void => {
        if (active) emit(doc.visibilityState === "hidden" || (event !== undefined && event.type !== "visibilitychange") ? "background" : "active");
      };
      const sources = [[doc, "visibilitychange"], [doc, "freeze"], [win, "pagehide"]] as const;
      for (const [source, type] of sources) source.addEventListener(type, report);
      queueMicrotask(report);
      return () => {
        active = false;
        for (const [source, type] of sources) source.removeEventListener(type, report);
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
