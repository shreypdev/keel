import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";
import { afterEach, describe, expect, it, vi } from "vitest";
import { browserAdapters } from "../src/adapters/browser.js";
import { FsErrorCodec, StorageErrorCodec } from "../src/adapters/codecs.js";
import { NEEDS_OPFS, opfsFs } from "../src/adapters/fs.js";
import { PortIds } from "../src/adapters/ids.js";
import { NEEDS_INDEXED_DB, indexedDbKv } from "../src/adapters/kv.js";
import { NEEDS_SECURE_CONTEXT, webCryptoSecureStore } from "../src/adapters/secure.js";
import { type Adapters, FsError, type KvAdapter, StorageError, fsErrorFrom } from "../src/adapters/types.js";
import type { UndraUnhandledError } from "../src/call-error.js";
import { UndraCore } from "../src/core.js";
import { PortStatus, UndraWriter, codecs, decodePortReply, decodeValue, encodePortReply, encodeValue } from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { captureLog, track } from "./support/harness.js";

/*
 * The shared storage failure-injection suite of ADR-049 (Swift and Kotlin have `StorageFailureTests`): every
 * method of `Kv` and `SecureStore` fails with each `StorageError` through the real port plumbing (the core's
 * `PortCall`, the runtime's dispatch, the `PortReply` bytes it answers with: status 1 and the encoded error),
 * an untyped throw is answered "unavailable" (status 2) with an error-level log naming the adapter, and the
 * browser adapters map what the platform throws onto the variants.
 */

type Override = { [K in keyof Adapters]?: Adapters[K] | null };

const NONE: Override = { http: null, timer: null, kv: null, secureStore: null, fs: null, connectivity: null, lifecycle: null };

async function setup(adapters: Override, onError?: (error: UndraUnhandledError) => void) {
  const fake = new FakeCoreTransport({ mode: "remote" });
  const log = captureLog();
  const core = track(
    await UndraCore.attach(fake, {
      expectedSchemaHash: SCHEMA,
      shared: false,
      adapters: { ...NONE, log, ...adapters },
      ...(onError && { onError }),
    }),
  );
  return { fake, core, log };
}

/** Every variant, with the bytes `undra-ports` encodes it as (u16 index, then the payload). */
const VARIANTS: ReadonlyArray<{ readonly error: StorageError; readonly index: number; readonly text?: string }> = [
  { error: new StorageError.Unavailable("needs IndexedDB"), index: 0, text: "needs IndexedDB" },
  { error: new StorageError.Full(), index: 1 },
  { error: new StorageError.Locked(), index: 2 },
  { error: new StorageError.Corrupt("bad bytes"), index: 3, text: "bad bytes" },
  { error: new StorageError.Io("disk on fire"), index: 4, text: "disk on fire" },
];

function expectedBytes(index: number, text?: string): Uint8Array {
  const w = new UndraWriter(8);
  w.writeU16(index);
  if (text !== undefined) w.writeStr(text);
  return w.finish();
}

const PORTS = [
  { name: "Kv", ids: PortIds.Kv, adapter: "kv" },
  { name: "SecureStore", ids: PortIds.SecureStore, adapter: "secureStore" },
] as const;

const METHODS = ["get", "set", "delete", "list"] as const;

/** The encoded arguments of `method`. */
function argsOf(method: (typeof METHODS)[number]): Uint8Array {
  const w = new UndraWriter(16);
  w.writeStr(method === "list" ? "prefix/" : "key");
  if (method === "set") w.writeBytes(new Uint8Array([1, 2, 3]));
  return w.finish();
}

/** A storage adapter whose every method rejects with what `fail` returns. */
function failing(fail: () => unknown): KvAdapter {
  return {
    get: () => Promise.reject(fail()),
    set: () => Promise.reject(fail()),
    delete: () => Promise.reject(fail()),
    list: () => Promise.reject(fail()),
  };
}

describe("StorageError", () => {
  it("encodes each variant with the numbering and payload of undra-ports, and decodes it back", () => {
    for (const { error, index, text } of VARIANTS) {
      const bytes = encodeValue(StorageErrorCodec, error);
      expect(bytes).toEqual(expectedBytes(index, text));
      const back = decodeValue(StorageErrorCodec, bytes);
      expect(back).toBeInstanceOf(error.constructor);
      expect(back.kind).toBe(error.kind);
      expect(back.message).toBe(error.message);
    }
  });

  it("has the messages of the Rust Display", () => {
    expect(VARIANTS.map((v) => v.error.message)).toEqual([
      "storage is unavailable: needs IndexedDB",
      "the storage is full",
      "the storage is locked",
      "stored data is corrupt: bad bytes",
      "storage I/O error: disk on fire",
    ]);
  });

  it("refuses an unknown variant index", () => {
    expect(() => decodeValue(StorageErrorCodec, expectedBytes(5))).toThrow(/invalid_tag|tag/);
  });

  it("from() maps what platforms throw", () => {
    expect(StorageError.from(new DOMException("over quota", "QuotaExceededError"))).toBeInstanceOf(StorageError.Full);
    expect(StorageError.from(Object.assign(new Error("no space left on device"), { code: "ENOSPC" }))).toBeInstanceOf(StorageError.Full);
    const security = StorageError.from(new DOMException("storage is disabled", "SecurityError"));
    expect(security).toBeInstanceOf(StorageError.Unavailable);
    expect((security as StorageError.Unavailable).value).toBe("storage is disabled");
    const other = StorageError.from(new DOMException("", "UnknownError"));
    expect(other).toBeInstanceOf(StorageError.Io);
    expect((other as StorageError.Io).value).toBe("UnknownError");
    const locked = new StorageError.Locked();
    expect(StorageError.from(locked)).toBe(locked);
    expect(StorageError.from("a string")).toEqual(new StorageError.Io("a string"));
  });
});

describe("every Kv and SecureStore method fails with each StorageError through the port plumbing", () => {
  for (const port of PORTS) {
    for (const method of METHODS) {
      for (const { error, index, text } of VARIANTS) {
        it(`${port.name}.${method} -> ${error.constructor.name}: status 1 with the encoded error`, async () => {
          const errors: UndraUnhandledError[] = [];
          const { fake, log } = await setup({ [port.adapter]: failing(() => error) }, (e) => errors.push(e));
          const reply = await fake.callPort(port.ids.portId, port.ids[method], argsOf(method));
          // The bytes the core receives: `port_call_id u32, status u8, body`.
          expect(encodePortReply(reply)).toEqual(
            encodePortReply({ portCallId: reply.portCallId, status: PortStatus.Error, body: expectedBytes(index, text) }),
          );
          expect(decodeValue(StorageErrorCodec, reply.body).kind).toBe(error.kind);
          // A typed failure is the adapter doing its job: nothing to report.
          expect(errors).toEqual([]);
          expect(log.records.filter((r) => r.level >= 4)).toEqual([]);
        });
      }
    }
  }

  for (const port of PORTS) {
    it(`${port.name}: a typed failure thrown before the adapter's first await is typed too`, async () => {
      const throwing: KvAdapter = {
        get: () => {
          throw new StorageError.Locked();
        },
        set: () => {
          throw new StorageError.Locked();
        },
        delete: () => {
          throw new StorageError.Locked();
        },
        list: () => {
          throw new StorageError.Locked();
        },
      };
      const { fake } = await setup({ [port.adapter]: throwing });
      for (const method of METHODS) {
        const reply = await fake.callPort(port.ids.portId, port.ids[method], argsOf(method));
        expect(reply.status).toBe(PortStatus.Error);
        expect(decodeValue(StorageErrorCodec, reply.body)).toBeInstanceOf(StorageError.Locked);
      }
    });

    it(`${port.name}: a raw QuotaExceededError or SecurityError is typed as Full or Unavailable`, async () => {
      let next: unknown = new DOMException("quota", "QuotaExceededError");
      const { fake } = await setup({ [port.adapter]: failing(() => next) });
      const full = await fake.callPort(port.ids.portId, port.ids.set, argsOf("set"));
      expect(full.status).toBe(PortStatus.Error);
      expect(decodeValue(StorageErrorCodec, full.body)).toBeInstanceOf(StorageError.Full);
      next = new DOMException("blocked by the user", "SecurityError");
      const unavailable = await fake.callPort(port.ids.portId, port.ids.get, argsOf("get"));
      expect(decodeValue(StorageErrorCodec, unavailable.body)).toEqual(new StorageError.Unavailable("blocked by the user"));
    });

    it(`${port.name}: an untyped throw is answered unavailable (status 2), logged at error level naming the adapter, and reported`, async () => {
      const errors: UndraUnhandledError[] = [];
      const { fake, log } = await setup({ [port.adapter]: failing(() => new Error("adapter bug")) }, (e) => errors.push(e));
      for (const method of METHODS) {
        const reply = await fake.callPort(port.ids.portId, port.ids[method], argsOf(method));
        expect(encodePortReply(reply)).toEqual(encodePortReply({ portCallId: reply.portCallId, status: PortStatus.Unavailable, body: new Uint8Array(0) }));
      }
      expect(errors.map((e) => e.operation.split(" (")[0])).toEqual(METHODS.map((m) => `${port.name}.${m} adapter`));
      const records = log.records.filter((r) => r.level === 4);
      expect(records).toHaveLength(4);
      expect(records[0]?.message).toContain(`${port.name}.get adapter`);
      expect(records[0]?.message).toContain("adapter bug");
      expect(records[0]?.message).toContain("typed error");
    });
  }

  it("the success path keeps its bodies (status 0, the same bytes as before ADR-049)", async () => {
    const memory = new Map<string, Uint8Array>();
    const kv: KvAdapter = {
      get: async (key) => memory.get(key) ?? null,
      set: async (key, value) => {
        memory.set(key, value);
      },
      delete: async (key) => {
        memory.delete(key);
      },
      list: async (prefix) => [...memory.keys()].filter((k) => k.startsWith(prefix)).sort(),
    };
    const { fake } = await setup({ kv });
    expect((await fake.callPort(PortIds.Kv.portId, PortIds.Kv.set, argsOf("set"))).body).toEqual(new Uint8Array(0));
    const got = await fake.callPort(PortIds.Kv.portId, PortIds.Kv.get, argsOf("get"));
    expect(got.status).toBe(PortStatus.Ok);
    expect(decodeValue(codecs.option(codecs.bytes), got.body)).toEqual(new Uint8Array([1, 2, 3]));
    const listed = await fake.callPort(PortIds.Kv.portId, PortIds.Kv.list, encodeValue(codecs.string, "k"));
    expect(decodeValue(codecs.vec(codecs.string), listed.body)).toEqual(["key"]);
    expect((await fake.callPort(PortIds.Kv.portId, PortIds.Kv.delete, argsOf("delete"))).status).toBe(PortStatus.Ok);
  });
});

describe("the browser storage adapters answer typed failures", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("without IndexedDB, the default Kv and SecureStore are registered anyway and answer Unavailable(needs IndexedDB)", async () => {
    expect((globalThis as { indexedDB?: unknown }).indexedDB).toBeUndefined();
    const defaults = browserAdapters();
    expect(defaults.kv).toBeDefined();
    expect(defaults.secureStore).toBeDefined();
    const { fake } = await setup({ kv: defaults.kv ?? null, secureStore: defaults.secureStore ?? null });
    for (const port of PORTS) {
      for (const method of METHODS) {
        const reply = await fake.callPort(port.ids.portId, port.ids[method], argsOf(method));
        expect(reply.status).toBe(PortStatus.Error);
        expect(decodeValue(StorageErrorCodec, reply.body)).toEqual(new StorageError.Unavailable(NEEDS_INDEXED_DB));
      }
    }
  });

  it("UndraCore.attach registers the storage ports by default, so a core in Node reads a reason, not 'no adapter'", async () => {
    const fake = new FakeCoreTransport({ mode: "remote" });
    const core = track(await UndraCore.attach(fake, { expectedSchemaHash: SCHEMA, shared: false, adapters: { log: captureLog() } }));
    expect(core.closed).toBe(false);
    const reply = await fake.callPort(PortIds.Kv.portId, PortIds.Kv.get, argsOf("get"));
    expect(decodeValue(StorageErrorCodec, reply.body)).toEqual(new StorageError.Unavailable(NEEDS_INDEXED_DB));
    const fs = await fake.callPort(PortIds.Fs.portId, PortIds.Fs.read, encodeValue(codecs.string, "a"));
    expect(fs.status).toBe(PortStatus.Error);
    expect(decodeValue(FsErrorCodec, fs.body)).toEqual(new FsError.Unavailable(NEEDS_OPFS));
  });

  it("without crypto.subtle, SecureStore answers Unavailable(needs a secure context), even where IndexedDB is missing too", async () => {
    const store = webCryptoSecureStore({ crypto: { getRandomValues: crypto.getRandomValues.bind(crypto) } as unknown as Crypto });
    for (const method of METHODS) {
      const call = method === "set" ? store.set("k", new Uint8Array(1)) : method === "list" ? store.list("") : store[method]("k");
      await expect(call).rejects.toEqual(new StorageError.Unavailable(NEEDS_SECURE_CONTEXT));
    }
  });

  describe("IndexedDB Kv (fake-indexeddb)", () => {
    const fresh = () => {
      vi.stubGlobal("IDBKeyRange", IDBKeyRange);
      return indexedDbKv({ indexedDB: new IDBFactory(), name: `kv-${Math.random()}` });
    };

    it("works, then maps a QuotaExceededError on write to Full", async () => {
      const kv = fresh();
      await kv.set("a", new Uint8Array([1]));
      expect(await kv.get("a")).toEqual(new Uint8Array([1]));
      expect(await kv.list("")).toEqual(["a"]);
      vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(() => {
        throw new DOMException("the quota has been exceeded", "QuotaExceededError");
      });
      await expect(kv.set("b", new Uint8Array([2]))).rejects.toBeInstanceOf(StorageError.Full);
    });

    it("maps a value that is not bytes to Corrupt and keeps the key", async () => {
      const factory = new IDBFactory();
      const name = `kv-${Math.random()}`;
      const kv = indexedDbKv({ indexedDB: factory, name });
      await kv.set("x", new Uint8Array([1]));
      // Something else wrote a string under the key.
      vi.spyOn(IDBObjectStore.prototype, "get").mockImplementationOnce(function (this: IDBObjectStore) {
        const request = { result: "not bytes", error: null } as unknown as IDBRequest;
        queueMicrotask(() => {
          (request as unknown as { onsuccess: () => void }).onsuccess();
        });
        return request;
      });
      const failure = await kv.get("x").catch((e: unknown) => e);
      expect(failure).toBeInstanceOf(StorageError.Corrupt);
      expect((failure as StorageError.Corrupt).value).toContain('"x"');
      expect(await kv.get("x")).toEqual(new Uint8Array([1]));
    });

    it("maps any other failure to Io with the browser's text", async () => {
      const kv = fresh();
      vi.spyOn(IDBObjectStore.prototype, "delete").mockImplementation(() => {
        throw new DOMException("the transaction is not active", "TransactionInactiveError");
      });
      await expect(kv.delete("a")).rejects.toEqual(new StorageError.Io("the transaction is not active"));
    });
  });

  describe("WebCrypto SecureStore", () => {
    /** An in-memory backing store and key store, so that the test can tamper with the ciphertext. */
    const inMemory = () => {
      const data = new Map<string, Uint8Array>();
      let key: CryptoKey | null = null;
      const kv: KvAdapter = {
        get: async (k) => data.get(k) ?? null,
        set: async (k, v) => {
          data.set(k, v);
        },
        delete: async (k) => {
          data.delete(k);
        },
        list: async (p) => [...data.keys()].filter((k) => k.startsWith(p)),
      };
      return {
        data,
        store: webCryptoSecureStore({
          kv,
          keyStore: {
            load: async () => key,
            save: async (k) => {
              key = k;
            },
          },
        }),
        loseKey: () => {
          key = null;
        },
      };
    };

    it("round-trips, then maps a tampered ciphertext (OperationError) to Corrupt", async () => {
      const { data, store } = inMemory();
      await store.set("token", new Uint8Array([7, 7]));
      expect(await store.get("token")).toEqual(new Uint8Array([7, 7]));
      const sealed = data.get("token") as Uint8Array;
      sealed[sealed.length - 1] = (sealed[sealed.length - 1] as number) ^ 0xff;
      const failure = await store.get("token").catch((e: unknown) => e);
      expect(failure).toBeInstanceOf(StorageError.Corrupt);
      expect((failure as StorageError).message).toContain("does not decrypt");
    });

    it("maps a value moved to another key (the key name is authenticated) to Corrupt", async () => {
      const { data, store } = inMemory();
      await store.set("a", new Uint8Array([1]));
      data.set("b", data.get("a") as Uint8Array);
      await expect(store.get("b")).rejects.toBeInstanceOf(StorageError.Corrupt);
    });

    it("maps a value not in the stored format to Corrupt", async () => {
      const { data, store } = inMemory();
      data.set("raw", new Uint8Array([9, 9, 9]));
      const failure = await store.get("raw").catch((e: unknown) => e);
      expect(failure).toBeInstanceOf(StorageError.Corrupt);
      expect((failure as StorageError).message).toContain("secure-store format");
    });

    it("passes a full backing store through as Full", async () => {
      const full: KvAdapter = { ...failing(() => new DOMException("quota", "QuotaExceededError")) };
      const store = webCryptoSecureStore({ kv: full, keyStore: { load: async () => null, save: async () => {} } });
      await expect(store.set("k", new Uint8Array(1))).rejects.toBeInstanceOf(StorageError.Full);
    });
  });
});

describe("FsError gains Full and Unavailable (ADR-049)", () => {
  it("encodes Full as 3 and Unavailable as 4 with its text", () => {
    expect(encodeValue(FsErrorCodec, new FsError.Full())).toEqual(expectedBytes(3));
    expect(encodeValue(FsErrorCodec, new FsError.Unavailable("no OPFS"))).toEqual(expectedBytes(4, "no OPFS"));
    expect(decodeValue(FsErrorCodec, expectedBytes(3))).toBeInstanceOf(FsError.Full);
    expect(decodeValue(FsErrorCodec, expectedBytes(4, "no OPFS"))).toEqual(new FsError.Unavailable("no OPFS"));
    expect(new FsError.Full().message).toBe("the disk is full");
    expect(new FsError.Unavailable("x").message).toBe("the file system is unavailable: x");
  });

  it("fsErrorFrom maps quota and ENOSPC to Full, and keeps the earlier mapping", () => {
    expect(fsErrorFrom(new DOMException("q", "QuotaExceededError"))).toBeInstanceOf(FsError.Full);
    expect(fsErrorFrom(Object.assign(new Error("no space"), { code: "ENOSPC" }))).toBeInstanceOf(FsError.Full);
    expect(fsErrorFrom(Object.assign(new Error("gone"), { code: "ENOENT" }))).toBeInstanceOf(FsError.NotFound);
    expect(fsErrorFrom(new DOMException("x", "NotFoundError"))).toBeInstanceOf(FsError.NotFound);
    expect(fsErrorFrom(new DOMException("x", "NotAllowedError"))).toBeInstanceOf(FsError.Denied);
    expect(fsErrorFrom(Object.assign(new Error("no"), { code: "EACCES" }))).toBeInstanceOf(FsError.Denied);
    expect(fsErrorFrom(new Error("other"))).toEqual(new FsError.Io("other"));
  });

  it("the OPFS adapter answers Unavailable where there is no origin private file system", async () => {
    await expect(opfsFs().read("a")).rejects.toEqual(new FsError.Unavailable(NEEDS_OPFS));
  });

  it("the Fs port answers a raw ENOSPC or QuotaExceededError as Full (status 1) and an untyped throw as unavailable", async () => {
    const errors: UndraUnhandledError[] = [];
    let next: unknown = Object.assign(new Error("no space left on device"), { code: "ENOSPC" });
    const { fake } = await setup(
      {
        fs: {
          read: () => Promise.reject(next),
          write: () => Promise.reject(next),
          delete: () => Promise.reject(next),
          list: () => Promise.reject(next),
        },
      },
      (e) => errors.push(e),
    );
    const write = new UndraWriter(8);
    write.writeStr("f");
    write.writeBytes(new Uint8Array([1]));
    const enospc = await fake.callPort(PortIds.Fs.portId, PortIds.Fs.write, write.finish());
    expect(decodePortReply(encodePortReply(enospc))).toEqual({ portCallId: enospc.portCallId, status: PortStatus.Error, body: expectedBytes(3) });
    next = new DOMException("q", "QuotaExceededError");
    expect((await fake.callPort(PortIds.Fs.portId, PortIds.Fs.read, encodeValue(codecs.string, "f"))).body).toEqual(expectedBytes(3));
    next = new Error("bug");
    expect((await fake.callPort(PortIds.Fs.portId, PortIds.Fs.read, encodeValue(codecs.string, "f"))).status).toBe(PortStatus.Unavailable);
    expect(errors.map((e) => e.operation.split(" (")[0])).toEqual(["Fs.read adapter"]);
  });
});
