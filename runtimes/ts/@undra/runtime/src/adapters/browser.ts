import { opfsFs } from "./fs.js";
import { fetchHttp } from "./http.js";
import { indexedDbKv } from "./kv.js";
import { webCryptoSecureStore } from "./secure.js";
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

/** Options of {@link browserAdapters}. */
export interface BrowserAdaptersOptions {
  /**
   * The namespace of the core the defaults are made for (`LoadOptions.namespace`, `UndraIds.namespace`): the stores of
   * `Kv`, `SecureStore` and `Fs` are per namespace, `undra.<namespace>.kv` and so on (SPEC 8, ADR-044 amendment A), so
   * two cores of one page never share them. Default `"_"`.
   */
  readonly namespace?: string | undefined;
}

/**
 * The default adapters of a browser (SPEC 11): `fetch` for Http, IndexedDB
 * for Kv, WebCrypto plus IndexedDB for SecureStore, the origin private file
 * system for Fs, `setTimeout` for Timer, `navigator.onLine` and Page
 * Visibility for Connectivity and Lifecycle, the console for Log.
 * The stores of Kv, SecureStore and Fs are per core namespace (`undra.<namespace>.kv`, ..., `undra/<namespace>/fs`). Safe to call
 * anywhere (Node, workers, tests).
 *
 * The storage ports are always present (ADR-049): where the platform lacks what
 * one needs, it answers every call with a typed failure that says so instead of
 * leaving the port unregistered, so the core reads a reason and not "no adapter":
 * Kv and SecureStore fail with `StorageError.Unavailable("needs IndexedDB")`,
 * SecureStore with `Unavailable("needs a secure context")` without
 * `crypto.subtle`, Fs with `FsError.Unavailable` without the origin private file
 * system. Http, Connectivity and Lifecycle are present only when the platform has
 * them. Clock and Rng are not listed; the wasm core has built-in bindings for them.
 */
export function browserAdapters(options: BrowserAdaptersOptions = {}): Partial<Adapters> {
  const { namespace } = options;
  const g = globalThis as {
    fetch?: unknown;
    navigator?: { onLine?: unknown };
    document?: { visibilityState?: unknown };
    addEventListener?: unknown;
  };
  const adapters: Partial<Adapters> = {
    timer: setTimeoutTimer(),
    log: consoleLog(),
    kv: indexedDbKv({ namespace }),
    secureStore: webCryptoSecureStore({ namespace }),
    fs: opfsFs({ namespace }),
  };
  if (typeof g.fetch === "function") adapters.http = fetchHttp();
  if (typeof g.navigator?.onLine === "boolean" && typeof g.addEventListener === "function") {
    adapters.connectivity = browserConnectivity();
  }
  if (g.document?.visibilityState !== undefined) adapters.lifecycle = browserLifecycle();
  return adapters;
}
