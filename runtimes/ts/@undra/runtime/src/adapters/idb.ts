import { msg } from "../messages.js";
/*
 * Small promise wrappers over IndexedDB, shared by the Kv and SecureStore
 * adapters. Not exported from the package.
 */

/** Opens (creating on first use) database `name` with one object store per entry of `stores`. */
export function openDatabase(factory: IDBFactory, name: string, stores: readonly string[]): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = factory.open(name, 1);
    request.onupgradeneeded = () => {
      const db = request.result;
      for (const store of stores) {
        if (!db.objectStoreNames.contains(store)) db.createObjectStore(store);
      }
    };
    request.onsuccess = () => {
      resolve(request.result);
    };
    request.onerror = () => {
      reject(request.error ?? new Error(msg(10, name)));
    };
    request.onblocked = () => {
      reject(new Error(msg(11, name)));
    };
  });
}

/** The result of a request, as a promise. */
export function result<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => {
      resolve(request.result);
    };
    request.onerror = () => {
      reject(request.error ?? new Error(msg(12)));
    };
  });
}

/** Resolves when the transaction has committed, rejects when it aborted. */
export function committed(tx: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => {
      resolve();
    };
    tx.onerror = () => {
      reject(tx.error ?? new Error(msg(13)));
    };
    tx.onabort = () => {
      reject(tx.error ?? new Error(msg(14)));
    };
  });
}

/** A stored byte value as a `Uint8Array` (structured clone hands back the type it was given, but be liberal). */
export function toBytes(value: unknown): Uint8Array {
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength).slice();
  throw new TypeError(msg(15));
}
