import type { PortImpl } from "../port.js";
import { opfsFs } from "./fs.js";
import { fetchHttp } from "./http.js";
import { indexedDbKv } from "./kv.js";
import { fsPort, httpPort, kvPort, secureStorePort } from "./ports.js";
import { webCryptoSecureStore } from "./secure.js";
import type { FsAdapter, HttpAdapter, KvAdapter } from "./types.js";

/*
 * What `default-ports.ts` loads on the first call to one of the default ports: the port implementations,
 * their codecs and error types, and the browser adapter behind each (used when the app gave none).
 */

/**
 * The implementation of the default port backed by `adapter` (its key in `AdapterOverrides`), or by the browser's when there is none;
 * the browser's storage adapters keep their data under the core's `namespace` (ADR-044 amendment A).
 */
export function standardPort(key: "http" | "kv" | "secureStore" | "fs", adapter: HttpAdapter | KvAdapter | FsAdapter | undefined, namespace?: string): PortImpl {
  switch (key) {
    case "http":
      return httpPort((adapter as HttpAdapter | undefined) ?? fetchHttp());
    case "kv":
      return kvPort((adapter as KvAdapter | undefined) ?? indexedDbKv({ namespace }));
    case "secureStore":
      return secureStorePort((adapter as KvAdapter | undefined) ?? webCryptoSecureStore({ namespace }));
    case "fs":
      return fsPort((adapter as FsAdapter | undefined) ?? opfsFs({ namespace }));
  }
}
