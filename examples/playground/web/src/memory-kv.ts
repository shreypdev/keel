import type { KvAdapter } from "@keel/runtime";

/**
 * The `Kv` port in memory. The core keeps its query cache and its offline queue in `Kv`; the
 * browser default is IndexedDB, which would outlive a reload while the playground's server (an
 * in-memory object, see `playground-server.ts`) would not. Both living exactly as long as the
 * page keeps them consistent.
 */
export function memoryKv(): KvAdapter {
  const entries = new Map<string, Uint8Array>();
  return {
    get: (key) => Promise.resolve(entries.get(key)?.slice() ?? null),
    set: (key, value) => {
      entries.set(key, value.slice());
      return Promise.resolve();
    },
    delete: (key) => {
      entries.delete(key);
      return Promise.resolve();
    },
    list: (prefix) => Promise.resolve([...entries.keys()].filter((key) => key.startsWith(prefix)).sort()),
  };
}
