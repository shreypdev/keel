import { indexedDbKv } from "./kv.js";
import { openDatabase, result } from "./idb.js";
import type { KvAdapter } from "./types.js";

/** Where the master key of {@link webCryptoSecureStore} is kept between sessions. */
export interface KeyStore {
  /** The stored key, or `null` when there is none yet. */
  load(): Promise<CryptoKey | null>;
  /** Stores the key. */
  save(key: CryptoKey): Promise<void>;
}

/** Options of {@link webCryptoSecureStore}. */
export interface WebCryptoSecureStoreOptions {
  /** Where the ciphertext lives. Default: an IndexedDB database named `"keel-secure"`. */
  readonly kv?: KvAdapter;
  /** Where the master key lives. Default: another IndexedDB database, `"keel-secure-keys"`. */
  readonly keyStore?: KeyStore;
  /** The WebCrypto implementation; default the global `crypto`. */
  readonly crypto?: Crypto;
  /** The IndexedDB implementation for the defaults; default the global `indexedDB`. */
  readonly indexedDB?: IDBFactory;
}

/** Format version of a stored value, its first byte. */
const FORMAT = 1;
const IV_BYTES = 12;
const ENCODER = new TextEncoder();

/** The default key store: an IndexedDB object store holding the non-extractable `CryptoKey` (browsers can structured-clone it). */
function indexedDbKeyStore(factory: IDBFactory | undefined): KeyStore {
  let database: Promise<IDBDatabase> | null = null;
  const open = (): Promise<IDBDatabase> => {
    if (factory === undefined) return Promise.reject(new Error("IndexedDB is not available on this platform"));
    return (database ??= openDatabase(factory, "keel-secure-keys", ["keys"]));
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
 */
export function webCryptoSecureStore(options: WebCryptoSecureStoreOptions = {}): KvAdapter {
  const factory = options.indexedDB ?? (globalThis as { indexedDB?: IDBFactory }).indexedDB;
  const kv = options.kv ?? indexedDbKv({ name: "keel-secure", store: "secure", ...(factory ? { indexedDB: factory } : {}) });
  const keyStore = options.keyStore ?? indexedDbKeyStore(factory);
  let master: Promise<CryptoKey> | null = null;

  const webcrypto = (): Crypto => {
    const c = options.crypto ?? (globalThis as { crypto?: Crypto }).crypto;
    if (c === undefined || c.subtle === undefined) throw new Error("WebCrypto is not available on this platform");
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

  const aad = (key: string): Uint8Array => ENCODER.encode(`keel.secure:${key}`);
  const asBuffer = (bytes: Uint8Array): BufferSource => bytes as unknown as BufferSource;

  return {
    async get(key) {
      const stored = await kv.get(key);
      if (stored === null) return null;
      if (stored.length < 1 + IV_BYTES + 16 || stored[0] !== FORMAT) {
        throw new Error(`the value stored under ${key} is not in the secure-store format`);
      }
      const iv = stored.subarray(1, 1 + IV_BYTES);
      const sealed = stored.subarray(1 + IV_BYTES);
      const plain = await webcrypto().subtle.decrypt(
        { name: "AES-GCM", iv: asBuffer(iv), additionalData: asBuffer(aad(key)) },
        await masterKey(),
        asBuffer(sealed),
      );
      return new Uint8Array(plain);
    },
    async set(key, value) {
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
    },
    delete: (key) => kv.delete(key),
    list: (prefix) => kv.list(prefix),
  };
}
