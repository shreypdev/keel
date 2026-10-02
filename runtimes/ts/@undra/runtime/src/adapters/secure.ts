import { NEEDS_INDEXED_DB, indexedDbKv } from "./kv.js";
import { openDatabase, result } from "./idb.js";
import { storeName } from "./names.js";
import { type KvAdapter, StorageError } from "./types.js";
import { msg } from "../messages.js";

/** Where the master key of {@link webCryptoSecureStore} is kept between sessions. */
export interface KeyStore {
  /** The stored key, or `null` when there is none yet. */
  load(): Promise<CryptoKey | null>;
  /** Stores the key. */
  save(key: CryptoKey): Promise<void>;
}

/** Options of {@link webCryptoSecureStore}. */
export interface WebCryptoSecureStoreOptions {
  /** Where the ciphertext lives. Default: an IndexedDB database named `"undra.<namespace>.secure"`. */
  readonly kv?: KvAdapter;
  /** Where the master key lives. Default: another IndexedDB database, `"undra.<namespace>.secure-keys"`. */
  readonly keyStore?: KeyStore;
  /** The namespace of the core the default databases are made for (SPEC 8, ADR-044 amendment A). Default `"_"`; ignored by a `kv` or `keyStore` you give. */
  readonly namespace?: string | undefined;
  /** The WebCrypto implementation; default the global `crypto`. */
  readonly crypto?: Crypto;
  /** The IndexedDB implementation for the defaults; default the global `indexedDB`. */
  readonly indexedDB?: IDBFactory;
}

/** The text of `StorageError.Unavailable` where the platform has no `crypto.subtle` (an insecure context, ADR-049). */
export const NEEDS_SECURE_CONTEXT = msg(21);

/** Format version of a stored value, its first byte. */
const FORMAT = 1;
const IV_BYTES = 12;
const ENCODER = new TextEncoder();

/** The default key store: an IndexedDB object store holding the non-extractable `CryptoKey` (browsers can structured-clone it). */
function indexedDbKeyStore(factory: IDBFactory | undefined, name: string): KeyStore {
  let database: Promise<IDBDatabase> | null = null;
  const open = (): Promise<IDBDatabase> => {
    if (factory === undefined) return Promise.reject(new StorageError.Unavailable(NEEDS_INDEXED_DB));
    return (database ??= openDatabase(factory, name, ["keys"]));
  };
  return {
    async load() {
      const db = await open();
      const key = await result(db.transaction("keys", "readonly").objectStore("keys").get("master"));
      return (key as CryptoKey | undefined) ?? null;
    },
    async save(key) {
      const db = await open();
      await result(db.transaction("keys", "readwrite").objectStore("keys").put(key, "master"));
    },
  };
}

/**
 * The `SecureStore` port for browsers: values are encrypted with AES-256-GCM
 * before they reach storage, under a master key that WebCrypto generates as
 * non-extractable and IndexedDB keeps. The key name is authenticated data, so
 * a ciphertext cannot be moved to another key.
 *
 * **This is weaker than a platform keystore** (Keychain, Android Keystore).
 * The key sits in the same origin storage as the data and any script running
 * on the origin can ask the browser to decrypt with it; the scheme only keeps
 * the stored bytes unreadable to someone who obtains the database files or a
 * backup of them. Do not treat it as protection against XSS. A value that
 * fails authentication (tampering, a lost key) makes `get` reject rather than
 * pretend the key is missing.
 *
 * Every failure is a {@link StorageError} (ADR-049): without `crypto.subtle` (a
 * page served over plain HTTP) every method rejects with `Unavailable("needs a
 * secure context")`, without IndexedDB with `Unavailable("needs IndexedDB")`; a
 * value that does not decrypt (`OperationError`) or is not in the stored format
 * with `Corrupt`, an exhausted quota with `Full`, anything else with `Io`.
 */
export function webCryptoSecureStore(options: WebCryptoSecureStoreOptions = {}): KvAdapter {
  const factory = options.indexedDB ?? (globalThis as { indexedDB?: IDBFactory | null }).indexedDB ?? undefined;
  const kv = options.kv ?? indexedDbKv({ name: storeName(options.namespace, "secure"), store: "secure", ...(factory ? { indexedDB: factory } : {}) });
  const keyStore = options.keyStore ?? indexedDbKeyStore(factory, storeName(options.namespace, "secure-keys"));
  let master: Promise<CryptoKey> | null = null;

  const webcrypto = (): Crypto => {
    const c = options.crypto ?? (globalThis as { crypto?: Crypto }).crypto;
    if (c === undefined || c.subtle === undefined || c.subtle === null) throw new StorageError.Unavailable(NEEDS_SECURE_CONTEXT);
    return c;
  };

  const masterKey = (): Promise<CryptoKey> => {
    master ??= (async () => {
      const existing = await keyStore.load();
      if (existing !== null) return existing;
      const key = await webcrypto().subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
      await keyStore.save(key);
      return key;
    })();
    master.catch(() => {
      master = null;
    });
    return master;
  };

  const aad = (key: string): Uint8Array => ENCODER.encode(`undra.secure:${key}`);
  const asBuffer = (bytes: Uint8Array): BufferSource => bytes as unknown as BufferSource;

  /**
   * Runs `work` with every failure as a {@link StorageError}. A missing `crypto.subtle` is the answer even where IndexedDB
   * is missing too; without IndexedDB, the stores below answer `Unavailable("needs IndexedDB")` themselves.
   */
  const storage = async <T>(work: () => Promise<T>): Promise<T> => {
    try {
      webcrypto();
      return await work();
    } catch (error) {
      throw StorageError.from(error);
    }
  };

  return {
    get: (key) =>
      storage(async () => {
        const stored = await kv.get(key);
        if (stored === null) return null;
        if (stored.length < 1 + IV_BYTES + 16 || stored[0] !== FORMAT) {
          throw new StorageError.Corrupt(msg(22, JSON.stringify(key)));
        }
        const iv = stored.subarray(1, 1 + IV_BYTES);
        const sealed = stored.subarray(1 + IV_BYTES);
        const master = await masterKey();
        let plain: ArrayBuffer;
        try {
          plain = await webcrypto().subtle.decrypt({ name: "AES-GCM", iv: asBuffer(iv), additionalData: asBuffer(aad(key)) }, master, asBuffer(sealed));
        } catch (error) {
          // AES-GCM fails authentication with an `OperationError`: tampered bytes, a value moved from another key, a lost master key.
          if ((error as { name?: unknown } | null)?.name === "OperationError") throw new StorageError.Corrupt(msg(23, JSON.stringify(key)));
          throw error;
        }
        return new Uint8Array(plain);
      }),
    set: (key, value) =>
      storage(async () => {
        const c = webcrypto();
        const iv = c.getRandomValues(new Uint8Array(IV_BYTES));
        const sealed = new Uint8Array(
          await c.subtle.encrypt({ name: "AES-GCM", iv: asBuffer(iv), additionalData: asBuffer(aad(key)) }, await masterKey(), asBuffer(value)),
        );
        const out = new Uint8Array(1 + IV_BYTES + sealed.length);
        out[0] = FORMAT;
        out.set(iv, 1);
        out.set(sealed, 1 + IV_BYTES);
        await kv.set(key, out);
      }),
    delete: (key) => storage(() => kv.delete(key)),
    list: (prefix) => storage(() => kv.list(prefix)),
  };
}
