import { committed, openDatabase, result, toBytes } from "./idb.js";
import { storeName } from "./names.js";
import { type KvAdapter, StorageError } from "./types.js";

/** Options of {@link indexedDbKv}. */
export interface IndexedDbKvOptions {
  /** Database name. Default `"undra.<namespace>.kv"` (SPEC 8, ADR-044 amendment A). */
  readonly name?: string;
  /** The namespace of the core the default database name is made for. Default `"_"`; ignored when `name` is given. */
  readonly namespace?: string | undefined;
  /** Object store name. Default `"kv"`. */
  readonly store?: string;
  /** The IndexedDB implementation; default the global `indexedDB`. */
  readonly indexedDB?: IDBFactory;
}

/** The text of `StorageError.Unavailable` where the platform has no IndexedDB (ADR-049). */
export const NEEDS_INDEXED_DB = "needs IndexedDB";

/** Runs `work`, turning whatever it throws into a {@link StorageError} (ADR-049's table). */
async function storage<T>(work: () => Promise<T>): Promise<T> {
  try {
    return await work();
  } catch (error) {
    throw StorageError.from(error);
  }
}

/**
 * The `Kv` port over IndexedDB: one object store keyed by the string key,
 * values as `Uint8Array`. Opened lazily on first use; writes resolve once the
 * transaction has committed.
 *
 * Every failure is a {@link StorageError} (ADR-049): without IndexedDB (Node, a
 * context that has none) every method rejects with `Unavailable("needs IndexedDB")`,
 * an exhausted quota with `Full`, a value that is not bytes with `Corrupt`, a
 * refused origin with `Unavailable`, anything else with `Io` and the browser's text.
 */
export function indexedDbKv(options: IndexedDbKvOptions = {}): KvAdapter {
  const name = options.name ?? storeName(options.namespace, "kv");
  const store = options.store ?? "kv";
  let database: Promise<IDBDatabase> | null = null;

  const open = (): Promise<IDBDatabase> => {
    if (database === null) {
      const factory = options.indexedDB ?? (globalThis as { indexedDB?: IDBFactory | null }).indexedDB;
      if (factory === undefined || factory === null) return Promise.reject(new StorageError.Unavailable(NEEDS_INDEXED_DB));
      database = openDatabase(factory, name, [store]);
      // A failed open must be retried by the next call, not cached.
      database.catch(() => {
        database = null;
      });
    }
    return database;
  };

  return {
    get: (key) =>
      storage(async () => {
        const db = await open();
        const value = await result(db.transaction(store, "readonly").objectStore(store).get(key));
        if (value === undefined) return null;
        try {
          return toBytes(value);
        } catch {
          throw new StorageError.Corrupt(`${JSON.stringify(key)} is not bytes`);
        }
      }),
    set: (key, value) =>
      storage(async () => {
        const db = await open();
        const tx = db.transaction(store, "readwrite");
        tx.objectStore(store).put(value.slice(), key);
        await committed(tx);
      }),
    delete: (key) =>
      storage(async () => {
        const db = await open();
        const tx = db.transaction(store, "readwrite");
        tx.objectStore(store).delete(key);
        await committed(tx);
      }),
    list: (prefix) =>
      storage(async () => {
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
      }),
  };
}
