import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { DbError } from "../src/adapters/types.js";
import { dbPort, nodeSqliteDb } from "../src/db.js";
import { cells, dbCalls, err, ok } from "./support/port-calls.js";

/*
 * Two cores in one process (ADR-044) each have their own `Db` binding (their own ids, queue and
 * transaction slot), and a database name is a file in the app's directory (ADR-048 §6), so two
 * cores that open the same name share its file by design. SQLite's own locks keep them apart: a
 * reader of one core does not see the other's uncommitted rows, and a write that meets the
 * other core's transaction waits SQLite's busy timeout and is then `Busy`, never a deadlock.
 */

let directory: string;

beforeAll(() => {
  directory = mkdtempSync(join(tmpdir(), "undra-db-two-"));
});

afterAll(() => {
  rmSync(directory, { recursive: true, force: true });
});

describe("two cores, one database name", () => {
  const schema = [{ version: 1, sql: "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)" }];

  it("share the file: committed rows are seen, uncommitted ones are not, and a contended write is Busy after the busy timeout", async () => {
    const a = dbCalls(dbPort(nodeSqliteDb({ directory })));
    const b = dbCalls(dbPort(nodeSqliteDb({ directory })));
    const da = ok(await a.open("shared", schema));
    const db = ok(await b.open("shared", schema));
    expect(da.version).toBe(1);
    expect(db.version, "the second core finds the first core's migration done").toBe(1);
    ok(await a.execute(da.db, "INSERT INTO t (id, v) VALUES (?, ?)", cells(1n, "a")));
    expect(ok(await b.query(db.db, "SELECT count(*) FROM t")).rows).toEqual([cells(1n)]);

    const tx = ok(await a.begin(da.db));
    ok(await a.execute(tx, "INSERT INTO t (id, v) VALUES (?, ?)", cells(2n, "b")));
    expect(ok(await b.query(db.db, "SELECT count(*) FROM t")).rows, "not the other core's uncommitted row").toEqual([cells(1n)]);
    const started = Date.now();
    const busy = err(await b.execute(db.db, "INSERT INTO t (id, v) VALUES (?, ?)", cells(3n, "c")));
    const waited = Date.now() - started;
    expect(busy).toBeInstanceOf(DbError.Busy);
    expect(waited, "SQLite's busy timeout (5 s), not at once").toBeGreaterThanOrEqual(4_500);
    expect(waited).toBeLessThan(8_000);

    ok(await a.commit(tx));
    expect(ok(await b.query(db.db, "SELECT count(*) FROM t")).rows).toEqual([cells(2n)]);
    ok(await b.execute(db.db, "INSERT INTO t (id, v) VALUES (?, ?)", cells(3n, "c")));
    expect(ok(await a.query(da.db, "SELECT count(*) FROM t")).rows).toEqual([cells(3n)]);
    ok(await a.close(da.db));
    ok(await b.close(db.db));
  }, 20_000);
});
