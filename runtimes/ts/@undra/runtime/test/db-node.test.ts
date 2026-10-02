import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { DbError } from "../src/adapters/types.js";
import { dbPort, nodeSqliteDb, sqliteError } from "../src/db.js";
import { dbSuite } from "./support/db-suite.js";
import { cells, dbCalls, err, ok } from "./support/port-calls.js";

/*
 * The Db suite of the brief (section 5) against the real Node adapter, `nodeSqliteDb` over
 * `node:sqlite`, through `dbPort`, rooted in a temporary directory: every constraint kind, Busy,
 * a corrupt file, unknown ids, invalid names, a refused downgrade, a failed migration rolled back,
 * typed cells (i64 extremes, reals, empty and binary blobs), the one-statement rule, the parameter
 * count, ":memory:", persistence.
 */

let directory: string;

beforeAll(() => {
  directory = mkdtempSync(join(tmpdir(), "undra-db-"));
});

afterAll(() => {
  rmSync(directory, { recursive: true, force: true });
});

dbSuite("nodeSqliteDb through dbPort", () => ({
  adapter: () => nodeSqliteDb({ directory }),
  wal: true,
  corrupt: (name) => {
    writeFileSync(join(directory, `${name}.sqlite`), "this is not a database, not at all, but long enough to have a header");
  },
  exists: (name) => existsSync(join(directory, `${name}.sqlite`)),
}));

describe("nodeSqliteDb: what dispose leaves on disk", () => {
  const migration = [{ version: 1, sql: "CREATE TABLE t (v INTEGER)" }];

  it("a transaction open when the core goes away is rolled back: a new connection gets the write lock at once and no row", async () => {
    const port = dbPort(nodeSqliteDb({ directory }));
    const api = dbCalls(port);
    const { db } = ok(await api.open("dangling", migration));
    ok(await api.execute(db, "INSERT INTO t VALUES (?)", cells(1n)));
    const tx = ok(await api.begin(db));
    ok(await api.execute(tx, "INSERT INTO t VALUES (?)", cells(2n)));
    port.dispose?.();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(err(await api.query(tx, "SELECT 1")), "the transaction is over").toBeInstanceOf(DbError.Unavailable);
    // A raw connection to the file: BEGIN IMMEDIATE takes the write lock without waiting.
    const sqlite = process.getBuiltinModule("node:sqlite") as typeof import("node:sqlite");
    const raw = new sqlite.DatabaseSync(join(directory, "dangling.sqlite"));
    try {
      raw.exec("PRAGMA busy_timeout = 0");
      raw.exec("BEGIN IMMEDIATE");
      expect(raw.prepare("SELECT v FROM t ORDER BY v").all(), "the committed row stays, the uncommitted one is gone").toEqual([{ v: 1 }]);
      raw.exec("ROLLBACK");
    } finally {
      raw.close();
    }
    const again = ok(await api.open("dangling", migration));
    expect(ok(await api.query(again.db, "SELECT COUNT(*) FROM t")).rows, "the port serves a new open after dispose").toEqual([cells(1n)]);
    ok(await api.close(again.db));
  });
});

describe("the error mapping", () => {
  it("maps result codes, never text", () => {
    expect(sqliteError(5, "x")).toEqual(new DbError.Busy());
    expect(sqliteError(6, "x")).toEqual(new DbError.Busy());
    expect(sqliteError(517, "x"), "SQLITE_BUSY_SNAPSHOT").toEqual(new DbError.Busy());
    expect(sqliteError(2067, "m")).toEqual(new DbError.Constraint("unique", "m"));
    expect(sqliteError(1555, "m")).toEqual(new DbError.Constraint("unique", "m"));
    expect(sqliteError(1299, "m")).toEqual(new DbError.Constraint("notNull", "m"));
    expect(sqliteError(787, "m")).toEqual(new DbError.Constraint("foreignKey", "m"));
    expect(sqliteError(275, "m")).toEqual(new DbError.Constraint("check", "m"));
    expect(sqliteError(1811, "m")).toEqual(new DbError.Constraint("other", "m"));
    expect(sqliteError(11, "m")).toEqual(new DbError.Corrupt("m"));
    expect(sqliteError(26, "m")).toEqual(new DbError.Corrupt("m"));
    expect(sqliteError(13, "m")).toEqual(new DbError.Full());
    for (const code of [14, 3, 8, 10, 266]) expect(sqliteError(code, "m"), String(code)).toEqual(new DbError.Unavailable("m"));
    expect(sqliteError(1, "m")).toEqual(new DbError.Sql("m"));
  });
});
