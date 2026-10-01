import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import SQLiteESMFactory from "wa-sqlite/dist/wa-sqlite.mjs";
import { MemoryVFS } from "wa-sqlite/src/examples/MemoryVFS.js";
import { Factory } from "wa-sqlite/src/sqlite-api.js";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { DbErrorCodec } from "../src/adapters/codecs.js";
import { DbError, type DbExecuted } from "../src/adapters/types.js";
import { type DbAdapter, type DbWorkerLike, type DbWorkerReply, type DbWorkerRequest, type DbWorkerScope, dbPort, serveDb, waSqliteDb, workerDbAdapter } from "../src/db.js";
import { startDbWorker, waSqliteAdapter } from "../src/db-worker.js";
import { waSqliteEngine } from "../src/db/wa-sqlite-engine.js";
import { decodeValue, encodeValue } from "../src/wire/index.js";
import { type DbSuiteTarget, dbSuite } from "./support/db-suite.js";
import { cells, dbCalls, err, ok } from "./support/port-calls.js";

/*
 * The browser's Db adapter (ADR-048 §7) on Node: the wa-sqlite engine `waSqliteDb` runs in its
 * worker, here over wa-sqlite's in-memory VFS (OPFS exists only in a browser's dedicated workers),
 * through the shared Db suite twice, directly and behind the worker protocol on a `MessageChannel`;
 * then the protocol's own cases with scripted workers, and the worker entry's loader.
 */

const WASM = readFileSync(createRequire(import.meta.url).resolve("wa-sqlite/dist/wa-sqlite.wasm"));

/** A MemoryVFS file record (wa-sqlite's example keeps them in `mapNameToFile`). */
interface MemoryFile {
  name: string;
  flags: number;
  size: number;
  data: ArrayBuffer;
}

/** An engine over a fresh wa-sqlite module and MemoryVFS, with the VFS's files at hand. */
async function memoryEngine(): Promise<{ readonly engine: DbAdapter; readonly files: Map<string, MemoryFile> }> {
  const module = await SQLiteESMFactory({ wasmBinary: WASM });
  const api = Factory(module);
  const vfs = new MemoryVFS();
  api.vfs_register(vfs, false);
  return { engine: waSqliteEngine(api, module, vfs.name, (name) => `/${name}`), files: (vfs as unknown as { mapNameToFile: Map<string, MemoryFile> }).mapNameToFile };
}

function target(engine: DbAdapter, files: Map<string, MemoryFile>): DbSuiteTarget {
  return {
    adapter: () => engine,
    wal: false,
    corrupt: (name) => {
      const garbage = new TextEncoder().encode("this is not a database, not at all, but long enough to have a header".repeat(80));
      files.set(`/${name}`, { name: `/${name}`, flags: 0x106, size: garbage.length, data: garbage.buffer });
    },
    exists: (name) => files.has(`/${name}`),
  };
}

/** Both ends of a worker served in this thread: `host` for `waSqliteDb({ worker })`, `scope` for `serveDb`. */
function channel(): { readonly host: DbWorkerLike; readonly scope: DbWorkerScope; close(): void } {
  const { port1, port2 } = new MessageChannel();
  port1.start();
  port2.start();
  return {
    host: port2 as unknown as DbWorkerLike,
    scope: port1 as unknown as DbWorkerScope,
    close() {
      port1.close();
      port2.close();
    },
  };
}

let direct: DbSuiteTarget;
let proxied: DbSuiteTarget;
const cleanups: Array<() => void> = [];

beforeAll(async () => {
  const a = await memoryEngine();
  direct = target(a.engine, a.files);
  const b = await memoryEngine();
  const line = channel();
  cleanups.push(serveDb(line.scope, b.engine), () => line.close());
  proxied = { ...target(b.engine, b.files), adapter: (() => {
    const adapter = waSqliteDb({ worker: line.host });
    return () => adapter;
  })() };
});

afterAll(() => {
  for (const cleanup of cleanups.splice(0)) cleanup();
});

dbSuite("the wa-sqlite engine (MemoryVFS) through dbPort", () => direct);
dbSuite("waSqliteDb: the engine in a worker, every call over postMessage", () => proxied);

/** A worker the test scripts: it records requests, and the test answers them or fails it. */
class ScriptedWorker implements DbWorkerLike {
  readonly requests: DbWorkerRequest[] = [];
  readonly #listeners = new Map<string, Array<(event: Event) => void>>();
  terminated = false;
  postMessage(message: DbWorkerRequest): void {
    this.requests.push(message);
  }
  addEventListener(type: string, listener: (event: Event) => void): void {
    this.#listeners.set(type, [...(this.#listeners.get(type) ?? []), listener]);
  }
  removeEventListener(): void {}
  terminate(): void {
    this.terminated = true;
  }
  fire(type: string, event: object): void {
    for (const listener of this.#listeners.get(type) ?? []) listener(event as Event);
  }
  reply(reply: DbWorkerReply): void {
    this.fire("message", { data: reply });
  }
}

const tick = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

describe("the Db worker protocol", () => {
  it("numbers requests and settles each caller with its own reply, in any order; errors arrive typed", async () => {
    const worker = new ScriptedWorker();
    const adapter = workerDbAdapter(() => worker);
    const opening = adapter.open("notes");
    await tick();
    expect(worker.requests).toEqual([{ t: "open", id: 1, name: "notes" }]);
    worker.reply({ t: "ok", id: 1, value: 7 });
    const connection = await opening;
    const first = connection.execute("INSERT 1", cells(1n));
    const second = connection.query("SELECT", []);
    await tick();
    expect(worker.requests.slice(1)).toEqual([
      { t: "execute", id: 2, conn: 7, sql: "INSERT 1", params: cells(1n) },
      { t: "query", id: 3, conn: 7, sql: "SELECT", params: [] },
    ]);
    worker.reply({ t: "ok", id: 3, value: { columns: ["n"], rows: [cells(2n)] } });
    worker.reply({ t: "error", id: 2, error: encodeValue(DbErrorCodec, new DbError.Constraint("unique", "UNIQUE constraint failed: t.id")) });
    expect(await second).toEqual({ columns: ["n"], rows: [cells(2n)] });
    const failure = await first.then(
      () => undefined,
      (error: unknown) => error,
    );
    expect(failure).toBeInstanceOf(DbError.Constraint);
    expect(failure).toMatchObject({ kind_: "unique", message_: "UNIQUE constraint failed: t.id" });
  });

  it("a worker that fails fails every waiting and later call with Unavailable", async () => {
    const worker = new ScriptedWorker();
    const adapter = workerDbAdapter(() => worker);
    const opening = adapter.open("a");
    await tick();
    worker.fire("error", { message: "Uncaught SyntaxError: bad worker script" });
    await expect(opening).rejects.toEqual(new DbError.Unavailable("the database worker failed: Uncaught SyntaxError: bad worker script"));
    await expect(adapter.open("b")).rejects.toEqual(new DbError.Unavailable("the database worker failed: Uncaught SyntaxError: bad worker script"));
    const unmade = workerDbAdapter(() => {
      throw new Error("no Worker here");
    });
    await expect(unmade.open("a")).rejects.toEqual(new DbError.Unavailable("the database worker could not start: no Worker here"));
  });

  it("serveDb answers an unknown connection and an untyped failure with Unavailable, and closes what it opened when stopped", async () => {
    const closed: string[] = [];
    const adapter: DbAdapter = {
      async open(name) {
        if (name === "boom") throw new RangeError("out of memory");
        return {
          execute: async (): Promise<DbExecuted> => ({ changes: 0n, lastInsertId: 0n }),
          query: async () => ({ columns: [], rows: [] }),
          executeScript: async () => {},
          close: async () => {
            closed.push(name);
          },
        };
      },
    };
    const line = channel();
    const stop = serveDb(line.scope, adapter);
    const host = workerDbAdapter(() => line.host);
    await expect(host.open("boom")).rejects.toEqual(new DbError.Unavailable("out of memory"));
    const connection = await host.open("kept");
    const replies: DbWorkerReply[] = [];
    line.host.addEventListener("message", (event) => {
      replies.push((event as MessageEvent).data as DbWorkerReply);
    });
    line.host.postMessage({ t: "query", id: 999, conn: 42, sql: "SELECT 1", params: [] });
    for (let i = 0; i < 100 && !replies.some((r) => r.id === 999); i++) await tick();
    const unknown = replies.find((r) => r.id === 999);
    expect(unknown?.t).toBe("error");
    expect(decodeValue(DbErrorCodec, (unknown as { error: Uint8Array }).error)).toEqual(new DbError.Unavailable("the database worker has no connection 42"));
    stop();
    expect(closed, "stopping closes the connections it opened").toEqual(["kept"]);
    void connection;
    line.close();
  });
});

describe("the worker entry (@undra/runtime/db-worker)", () => {
  it("waSqliteAdapter({ storage: \"memory\" }) loads wa-sqlite by itself (its wasm from the package under Node)", async () => {
    const api = dbCalls(dbPort(waSqliteAdapter({ storage: "memory" }), { wal: false }));
    const { db, version } = ok(await api.open("loaded", [{ version: 1, sql: "CREATE TABLE n (v INTEGER)" }]));
    expect(version).toBe(1);
    ok(await api.execute(db, "INSERT INTO n VALUES (?)", cells(2n ** 62n)));
    expect(ok(await api.query(db, "SELECT v FROM n")).rows).toEqual([cells(2n ** 62n)]);
  });

  it("OPFS (the default) where there is none is Unavailable, saying why", async () => {
    const failure = err(await dbCalls(dbPort(waSqliteAdapter(), { wal: false })).open("notes"));
    expect(failure).toBeInstanceOf(DbError.Unavailable);
    expect((failure as DbError.Unavailable).value).toContain("wa-sqlite could not start (opfs)");
  });

  it("startDbWorker serves waSqliteDb end to end", async () => {
    const line = channel();
    const stop = startDbWorker(line.scope, { storage: "memory" });
    const api = dbCalls(dbPort(waSqliteDb({ worker: () => line.host }), { wal: false }));
    const { db } = ok(await api.open("served"));
    ok(await api.execute(db, "CREATE TABLE t (b BLOB)"));
    ok(await api.execute(db, "INSERT INTO t VALUES (?)", cells(Uint8Array.of(1, 2, 3))));
    expect(ok(await api.query(db, "SELECT b, typeof(b) FROM t")).rows).toEqual([cells(Uint8Array.of(1, 2, 3), "blob")]);
    ok(await api.close(db));
    stop();
    line.close();
  });
});
