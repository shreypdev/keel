import type { KvAdapter } from "@keel/runtime";

/**
 * The `Kv` port in memory. The core keeps its query cache and its offline queue in `Kv`. The
 * browser's default is IndexedDB, which outlives a reload, while the playground's server (an
 * in-memory object, see `playground-server.ts`) does not: after a reload the core would show
 * cached items the server has forgotten. Keeping both in memory makes a reload start clean.
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
