import { IDBFactory } from "fake-indexeddb";
import { afterEach, describe, expect, it, vi } from "vitest";
import { browserAdapters } from "../src/adapters/browser.js";
import { opfsFs } from "../src/adapters/fs.js";
import { PortIds } from "../src/adapters/ids.js";
import { OptInPortIds } from "../src/adapters/opt-in-ids.js";
import { indexedDbKv } from "../src/adapters/kv.js";
import { UNNAMED_NAMESPACE, storeName, storePath } from "../src/adapters/names.js";
import { webCryptoSecureStore } from "../src/adapters/secure.js";
import { UndraCore } from "../src/core.js";
import { type DbAdapter, type DbConnection, dbPort } from "../src/db.js";
import { serveDb, workerDbAdapter, type DbWorkerLike, type DbWorkerReply, type DbWorkerRequest } from "../src/db/protocol.js";
import { UndraWriter } from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { dbCalls, ok } from "./support/port-calls.js";
import { track } from "./support/harness.js";

/*
 * ADR-044 amendment A: the default `Kv`, `SecureStore`, `Fs` and `Db` stores of the web are per core namespace, so two
 * cores in one page (one origin) never share one unless the app gives them an adapter of its own.
 */

afterEach(() => {
  vi.unstubAllGlobals();
});

const text = (bytes: Uint8Array | null): string | null => (bytes === null ? null : new TextDecoder().decode(bytes));
const bytes = (value: string): Uint8Array => new TextEncoder().encode(value);

describe("the names", () => {
  it("are undra.<namespace>.<store> in IndexedDB and undra/<namespace>/<store> in the origin private file system", () => {
    expect(storeName("playground_a", "kv")).toBe("undra.playground_a.kv");
    expect(storeName("playground_a", "secure-keys")).toBe("undra.playground_a.secure-keys");
    expect(storePath("playground_a", "fs")).toBe("undra/playground_a/fs");
    expect(storePath("playground_a", "db")).toBe("undra/playground_a/db");
    expect(storeName(undefined, "kv")).toBe("undra._.kv");
    expect(UNNAMED_NAMESPACE).toBe("_");
    expect(/^[a-z]/.test(UNNAMED_NAMESPACE), "no real namespace is `_`: they start with a lowercase letter").toBe(false);
  });
});

describe("Kv", () => {
  it("of two namespaces never sees the other's keys, and keeps them in two databases", async () => {
    const factory = new IDBFactory();
    const a = indexedDbKv({ namespace: "playground_a", indexedDB: factory });
    const b = indexedDbKv({ namespace: "playground_b", indexedDB: factory });
    await a.set("k", bytes("from a"));
    await b.set("k", bytes("from b"));
    await a.set("only-a", bytes("x"));
    expect(text(await a.get("k"))).toBe("from a");
    expect(text(await b.get("k"))).toBe("from b");
    expect(await b.get("only-a")).toBeNull();
    expect(await a.list("")).toEqual(["k", "only-a"]);
    expect(await b.list("")).toEqual(["k"]);
    const names = (await factory.databases()).map((d) => d.name).sort();
    expect(names).toEqual(["undra.playground_a.kv", "undra.playground_b.kv"]);
  });

  it("with a name of its own keeps it, whatever the namespace", async () => {
    const factory = new IDBFactory();
    const own = indexedDbKv({ name: "my-app-kv", namespace: "playground_a", indexedDB: factory });
    await own.set("k", bytes("v"));
    expect((await factory.databases()).map((d) => d.name)).toEqual(["my-app-kv"]);
  });
});

describe("SecureStore", () => {
  it("of two namespaces keeps its ciphertext and its master key in databases of their own", async () => {
    const factory = new IDBFactory();
    const a = webCryptoSecureStore({ namespace: "playground_a", indexedDB: factory });
    const b = webCryptoSecureStore({ namespace: "playground_b", indexedDB: factory });
    await a.set("token", bytes("secret a"));
    await b.set("token", bytes("secret b"));
    expect(text(await a.get("token"))).toBe("secret a");
    expect(text(await b.get("token"))).toBe("secret b");
    expect(await b.list("")).toEqual(["token"]);
    const names = (await factory.databases()).map((d) => d.name).sort();
    expect(names).toEqual([
      "undra.playground_a.secure",
      "undra.playground_a.secure-keys",
      "undra.playground_b.secure",
      "undra.playground_b.secure-keys",
    ]);
  });
});

/** A directory of an in-memory origin private file system: just what `opfsFs` calls. */
class MemoryDirectory {
  readonly directories = new Map<string, MemoryDirectory>();
  readonly files = new Map<string, Uint8Array>();

  async getDirectoryHandle(name: string, options?: { create?: boolean }): Promise<MemoryDirectory> {
    const found = this.directories.get(name);
    if (found !== undefined) return found;
    if (options?.create !== true) throw new DOMException("not found", "NotFoundError");
    const made = new MemoryDirectory();
    this.directories.set(name, made);
    return made;
  }

  async getFileHandle(name: string, options?: { create?: boolean }) {
    if (!this.files.has(name)) {
      if (options?.create !== true) throw new DOMException("not found", "NotFoundError");
      this.files.set(name, new Uint8Array(0));
    }
    return {
      getFile: async () => ({ arrayBuffer: async () => (this.files.get(name) as Uint8Array).slice().buffer }),
      createWritable: async () => ({
        write: async (data: Uint8Array) => {
          this.files.set(name, data.slice());
        },
        close: async () => undefined,
      }),
    };
  }

  async removeEntry(name: string): Promise<void> {
    this.files.delete(name);
    this.directories.delete(name);
  }

  async *keys(): AsyncGenerator<string> {
    yield* this.directories.keys();
    yield* this.files.keys();
  }
}

describe("Fs", () => {
  it("of two namespaces is the directory undra/<namespace>/fs of the origin, and the files are not shared", async () => {
    const origin = new MemoryDirectory();
    vi.stubGlobal("navigator", { storage: { getDirectory: async () => origin } });
    const a = opfsFs({ namespace: "playground_a" });
    const b = opfsFs({ namespace: "playground_b" });
    await a.write("notes/a.txt", bytes("a"));
    await b.write("notes/a.txt", bytes("b"));
    expect(text(await a.read("notes/a.txt"))).toBe("a");
    expect(text(await b.read("notes/a.txt"))).toBe("b");
    await a.write("only-a.txt", bytes("x"));
    await expect(b.read("only-a.txt")).rejects.toMatchObject({ constructor: expect.anything() });
    expect([...origin.directories.keys()]).toEqual(["undra"]);
    const undra = await origin.getDirectoryHandle("undra");
    expect([...undra.directories.keys()].sort()).toEqual(["playground_a", "playground_b"]);
    const fsOfA = await (await undra.getDirectoryHandle("playground_a")).getDirectoryHandle("fs");
    expect([...fsOfA.files.keys()]).toEqual(["only-a.txt"]);
    expect([...fsOfA.directories.keys()]).toEqual(["notes"]);
  });

  it("over a root of the app's own is not touched", async () => {
    const root = new MemoryDirectory();
    const own = opfsFs({ root: root as unknown as FileSystemDirectoryHandle, namespace: "playground_a" });
    await own.write("f.txt", bytes("x"));
    expect([...root.files.keys()]).toEqual(["f.txt"]);
  });
});

describe("browserAdapters and the core", () => {
  it("make every default from the namespace, and a core knows its own", async () => {
    const factory = new IDBFactory();
    vi.stubGlobal("indexedDB", factory);
    const adapters = browserAdapters({ namespace: "playground_a" });
    await adapters.kv?.set("k", bytes("v"));
    await adapters.secureStore?.set("s", bytes("v"));
    expect((await factory.databases()).map((d) => d.name).sort()).toEqual([
      "undra.playground_a.kv",
      "undra.playground_a.secure",
      "undra.playground_a.secure-keys",
    ]);

    const named = track(await UndraCore.attach(new FakeCoreTransport(), { expectedSchemaHash: SCHEMA, shared: false, namespace: "playground_b" }));
    const unnamed = track(await UndraCore.attach(new FakeCoreTransport(), { expectedSchemaHash: SCHEMA, shared: false }));
    expect(named.namespace).toBe("playground_b");
    expect(unnamed.namespace).toBe(UNNAMED_NAMESPACE);
  });

  it("serve the two cores of one page from two stores: a key set through one core's Kv port is not seen through the other's", async () => {
    const factory = new IDBFactory();
    vi.stubGlobal("indexedDB", factory);
    const load = async (namespace: string) => {
      const fake = new FakeCoreTransport({ mode: "remote" });
      track(
        await UndraCore.attach(fake, {
          expectedSchemaHash: SCHEMA,
          shared: false,
          namespace,
          adapters: { http: null, timer: null, secureStore: null, fs: null, connectivity: null, lifecycle: null },
        }),
      );
      return fake;
    };
    const [a, b] = [await load("playground_a"), await load("playground_b")];
    const set = (key: string, value: string): Uint8Array => {
      const w = new UndraWriter(16);
      w.writeStr(key);
      w.writeBytes(bytes(value));
      return w.finish();
    };
    const get = (key: string): Uint8Array => {
      const w = new UndraWriter(8);
      w.writeStr(key);
      return w.finish();
    };
    await a.callPort(PortIds.Kv.portId, PortIds.Kv.set, set("k", "value-of-a"));
    await b.callPort(PortIds.Kv.portId, PortIds.Kv.set, set("k", "value-of-b"));
    const fromA = text((await a.callPort(PortIds.Kv.portId, PortIds.Kv.get, get("k"))).body) ?? "";
    const fromB = text((await b.callPort(PortIds.Kv.portId, PortIds.Kv.get, get("k"))).body) ?? "";
    expect(fromA).toContain("value-of-a");
    expect(fromA).not.toContain("value-of-b");
    expect(fromB).toContain("value-of-b");
    expect(fromB).not.toContain("value-of-a");
    expect((await factory.databases()).map((d) => d.name).sort()).toEqual(["undra.playground_a.kv", "undra.playground_b.kv"]);
  });
});

/** An adapter that records the scope it is opened with. */
function recording(): { adapter: DbAdapter; opened: Array<{ name: string; namespace: string | undefined }> } {
  const opened: Array<{ name: string; namespace: string | undefined }> = [];
  const connection: DbConnection = {
    execute: async () => ({ changes: 0n, lastInsertId: 0n }),
    query: async () => ({ columns: [], rows: [] }),
    executeScript: async () => undefined,
    close: async () => undefined,
  };
  return {
    opened,
    adapter: {
      open: async (name, scope) => {
        opened.push({ name, namespace: scope?.namespace });
        return connection;
      },
    },
  };
}

describe("Db", () => {
  it("is told its core's namespace when the core registers the port, and opens every database for it", async () => {
    const { adapter, opened } = recording();
    const port = dbPort(adapter, { wal: false });
    const api = dbCalls(port);
    ok(await api.open("before-bind", []));
    expect(opened.at(-1)).toEqual({ name: "before-bind", namespace: undefined });

    const core = track(await UndraCore.attach(new FakeCoreTransport(), { expectedSchemaHash: SCHEMA, shared: false, namespace: "playground_a" }));
    core.registerPort(OptInPortIds.Db.portId, port);
    ok(await api.open("after-bind", []));
    expect(opened.at(-1)).toEqual({ name: "after-bind", namespace: "playground_a" });
  });

  it("crosses to the worker with the namespace of the core, which a worker serving the adapter receives", async () => {
    const { adapter, opened } = recording();
    const { port1, port2 } = new MessageChannel();
    const worker = {
      postMessage: (message: DbWorkerRequest) => port1.postMessage(message),
      addEventListener: (type: string, listener: (event: Event) => void) => {
        port1.addEventListener(type as "message", listener as (event: MessageEvent) => void);
        port1.start();
      },
      removeEventListener: (type: string, listener: (event: Event) => void) => {
        port1.removeEventListener(type as "message", listener as (event: MessageEvent) => void);
      },
    } satisfies DbWorkerLike;
    const stop = serveDb(
      {
        postMessage: (reply: DbWorkerReply) => port2.postMessage(reply),
        addEventListener: (_type, listener) => {
          port2.addEventListener("message", listener as (event: MessageEvent) => void);
          port2.start();
        },
        removeEventListener: (_type, listener) => {
          port2.removeEventListener("message", listener as (event: MessageEvent) => void);
        },
      },
      adapter,
    );
    try {
      const proxy = workerDbAdapter(() => worker);
      await (await proxy.open("notes", { namespace: "playground_b" })).close();
      await (await proxy.open("plain")).close();
      expect(opened).toEqual([
        { name: "notes", namespace: "playground_b" },
        { name: "plain", namespace: undefined },
      ]);
    } finally {
      stop();
      port1.close();
      port2.close();
    }
  });
});

/** What an app can hand `load` or `attach` as a namespace, and no core has (review of ns-storage): each becomes a path component. */
const BAD_NAMESPACES: ReadonlyArray<readonly [string, string]> = [
  ["..", ".."],
  [".", "."],
  ["a/b", "a/b"],
  ["a\\b", "a\\b"],
  ["../escape", "../escape"],
  ["", "(empty)"],
  ["a".repeat(33), "33 characters"],
  ["Upper", "uppercase"],
  ["1abc", "starts with a digit"],
  ["_hidden", "starts with `_`"],
  ["__", "two underscores"],
  ["café", "unicode"],
  ["a\u0000b", "a NUL byte"],
  ["with space", "a space"],
  ["with-dash", "a dash (the Android database file's separator)"],
  ["a.b", "a dot"],
];

describe("a namespace an app supplies", () => {
  it("of 32 lowercase letters, digits and `_` is accepted, as is none", async () => {
    vi.stubGlobal("indexedDB", new IDBFactory());
    for (const namespace of ["a", "playground_core", "a".repeat(32), "a1_b2"]) {
      const core = track(await UndraCore.attach(new FakeCoreTransport(), { expectedSchemaHash: SCHEMA, shared: false, namespace }));
      expect(core.namespace).toBe(namespace);
    }
  });

  for (const [namespace, why] of BAD_NAMESPACES) {
    it(`is refused typed at attach before it names a store: ${why}`, async () => {
      const factory = new IDBFactory();
      vi.stubGlobal("indexedDB", factory);
      const transport = new FakeCoreTransport();
      await expect(UndraCore.attach(transport, { expectedSchemaHash: SCHEMA, shared: false, namespace })).rejects.toMatchObject({
        name: "UndraError",
        kind: "options",
        message: expect.stringContaining("namespace"),
      });
      expect(transport.sent ?? [], "the transport was not touched").toEqual([]);
      expect(await factory.databases(), "no database was created").toEqual([]);
    });

    it(`is refused typed at load before anything is created: ${why}`, async () => {
      const factory = new IDBFactory();
      vi.stubGlobal("indexedDB", factory);
      await expect(
        UndraCore.load({ mode: "remote", url: "ws://localhost:1", expectedSchemaHash: SCHEMA, namespace }),
      ).rejects.toMatchObject({ kind: "options", message: expect.stringContaining("namespace") });
      expect(await factory.databases()).toEqual([]);
    });

    it(`never becomes a store name through an adapter either: ${why}`, () => {
      expect(() => storeName(namespace, "kv")).toThrow(/namespace/);
      expect(() => storePath(namespace, "fs")).toThrow(/namespace/);
      expect(() => indexedDbKv({ namespace })).toThrow(/namespace/);
      expect(() => webCryptoSecureStore({ namespace })).toThrow(/namespace/);
      expect(() => browserAdapters({ namespace })).toThrow(/namespace/);
      expect(() => opfsFs({ namespace })).toThrow(/namespace/);
    });
  }

  it("of `_` is the unnamed core's own: accepted, and the same stores as none", async () => {
    vi.stubGlobal("indexedDB", new IDBFactory());
    const core = track(await UndraCore.attach(new FakeCoreTransport(), { expectedSchemaHash: SCHEMA, shared: false, namespace: "_" }));
    expect(core.namespace).toBe(UNNAMED_NAMESPACE);
    expect(storeName("_", "kv")).toBe(storeName(undefined, "kv"));
  });

  it("is refused when it is not a string at all", async () => {
    vi.stubGlobal("indexedDB", new IDBFactory());
    for (const namespace of [42, null, {}, ["a"], 1n]) {
      await expect(
        UndraCore.attach(new FakeCoreTransport(), { expectedSchemaHash: SCHEMA, shared: false, namespace: namespace as unknown as string }),
      ).rejects.toMatchObject({ kind: "options" });
    }
  });

  it("is refused by the database worker's adapter as a typed Unavailable, not a path", async () => {
    const { waSqliteAdapter } = await import("../src/db-worker.js");
    const adapter = waSqliteAdapter({ storage: "opfs" });
    for (const namespace of ["..", "a/b", ""]) {
      await expect(adapter.open("notes", { namespace })).rejects.toMatchObject({ kind: "unavailable" });
    }
  });

  it("a long, odd one is shown shortened in the message", async () => {
    vi.stubGlobal("indexedDB", new IDBFactory());
    const error = await UndraCore.attach(new FakeCoreTransport(), { expectedSchemaHash: SCHEMA, shared: false, namespace: "x".repeat(5000) }).then(
      () => new Error("accepted"),
      (e: unknown) => e as Error,
    );
    expect(error.message).toContain("is not [a-z][a-z0-9_]{0,31}");
    expect(error.message.length).toBeLessThan(600);
  });
});
