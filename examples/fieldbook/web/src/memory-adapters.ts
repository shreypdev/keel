import type { FsAdapter, KvAdapter } from "@undra/runtime";

/** The `Kv` (and `SecureStore`) port in memory: what a test gives a core that has no IndexedDB. */
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

/** The `Fs` port in memory. */
export function memoryFs(): FsAdapter {
  const files = new Map<string, Uint8Array>();
  return {
    read: (path) => {
      const bytes = files.get(path);
      return bytes === undefined ? Promise.reject(new Error(`no such file: ${path}`)) : Promise.resolve(bytes.slice());
    },
    write: (path, data) => {
      files.set(path, data.slice());
      return Promise.resolve();
    },
    delete: (path) => {
      files.delete(path);
      return Promise.resolve();
    },
    list: (dir) => Promise.resolve([...files.keys()].filter((path) => path.startsWith(`${dir}/`)).sort()),
  };
}
