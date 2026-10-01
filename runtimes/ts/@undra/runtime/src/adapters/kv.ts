import { committed, openDatabase, result, toBytes } from "./idb.js";
import type { KvAdapter } from "./types.js";

/** Options of {@link indexedDbKv}. */
export interface IndexedDbKvOptions {
  /** Database name. Default `"undra-kv"`. */
  readonly name?: string;
  /** Object store name. Default `"kv"`. */
  readonly store?: string;
  /** The IndexedDB implementation; default the global `indexedDB`. */
  readonly indexedDB?: IDBFactory;
}

/**
 * The `Kv` port over IndexedDB: one object store keyed by the string key,
 * values as `Uint8Array`. Opened lazily on first use; writes resolve once the
 * transaction has committed.
 */
export function indexedDbKv(options: IndexedDbKvOptions = {}): KvAdapter {
  const name = options.name ?? "undra-kv";
  const store = options.store ?? "kv";
  let database: Promise<IDBDatabase> | null = null;

  const open = (): Promise<IDBDatabase> => {
    if (database === null) {
      const factory = options.indexedDB ?? (globalThis as { indexedDB?: IDBFactory }).indexedDB;
      if (factory === undefined) return Promise.reject(new Error("IndexedDB is not available on this platform"));
      database = openDatabase(factory, name, [store]);
      // A failed open must be retried by the next call, not cached.
      database.catch(() => {
        database = null;
      });
    }
    return database;
  };

  return {
    async get(key) {
      const db = await open();
      const value = await result(db.transaction(store, "readonly").objectStore(store).get(key));
      return value === undefined ? null : toBytes(value);
    },
    async set(key, value) {
      const db = await open();
      const tx = db.transaction(store, "readwrite");
      tx.objectStore(store).put(value.slice(), key);
      await committed(tx);
    },
    async delete(key) {
      const db = await open();
      const tx = db.transaction(store, "readwrite");
      tx.objectStore(store).delete(key);
      await committed(tx);
    },
    async list(prefix) {
      const db = await open();
      const objectStore = db.transaction(store, "readonly").objectStore(store);
      return new Promise<string[]>((resolve, reject) => {
        const keys: string[] = [];
        // Keys sort by UTF-16 code units, so the keys with a prefix are contiguous from the prefix on.
        const request = objectStore.openKeyCursor(prefix === "" ? null : IDBKeyRange.lowerBound(prefix));
        request.onsuccess = () => {
          const cursor = request.result;
          if (cursor === null) {
            resolve(keys);
            return;
          }
          const key = String(cursor.key);
          if (!key.startsWith(prefix)) {
            resolve(keys);
            return;
          }
          keys.push(key);
          cursor.continue();
        };
        request.onerror = () => {
          reject(request.error ?? new Error("IndexedDB cursor failed"));
        };
      });
    },
  };
}
