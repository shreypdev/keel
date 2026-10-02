import { opfsFs } from "./fs.js";
import { fetchHttp } from "./http.js";
import { indexedDbKv } from "./kv.js";
import { lightAdapters } from "./browser-events.js";
import { webCryptoSecureStore } from "./secure.js";
import type { Adapters } from "./types.js";

export {
  type BrowserConnectivityOptions,
  type DocumentLike,
  type NavigatorLike,
  browserConnectivity,
  browserLifecycle,
} from "./browser-events.js";

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
  const adapters: Partial<Adapters> = {
    ...lightAdapters(),
    kv: indexedDbKv({ namespace }),
    secureStore: webCryptoSecureStore({ namespace }),
    fs: opfsFs({ namespace }),
  };
  if (typeof (globalThis as { fetch?: unknown }).fetch === "function") adapters.http = fetchHttp();
  return adapters;
}
