import { describe, expect, it } from "vitest";
import { DbError, type DbValue } from "../../src/adapters/types.js";
import { type DbAdapter, dbPort } from "../../src/db.js";
import { cells, dbCalls, err, ok } from "./port-calls.js";

/*
 * The Db suite of the brief (section 5), run against each real adapter through `dbPort`: every
 * constraint kind, Busy, a corrupt file, unknown ids, invalid names, a refused downgrade, a failed
 * migration rolled back, typed cells (i64 extremes, reals, empty and binary blobs), the
 * one-statement rule, the parameter count, ":memory:", persistence across a reopen.
 */

/** The adapter under test and what the suite needs to know about its storage. */
export interface DbSuiteTarget {
  /** A port-level adapter over the target's storage; every call must see what earlier ones wrote. */
  adapter(): DbAdapter;
  /** Whether the target has WAL (`dbPort`'s `wal`). */
  readonly wal: boolean;
  /** Puts bytes that are not a database where database `name` lives. */
  corrupt(name: string): void;
  /** Whether a file for database `name` exists in the storage. */
  exists(name: string): boolean;
}

const SCHEMA = [
  {
    version: 1,
    sql: `CREATE TABLE parent (id INTEGER PRIMARY KEY);
          CREATE TABLE t (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            n INTEGER CHECK (n IS NULL OR n > 0),
            parent INTEGER REFERENCES parent (id)
          );
          CREATE TRIGGER no_bob BEFORE INSERT ON t WHEN NEW.name = 'bob' BEGIN SELECT RAISE(ABORT, 'no bob'); END;`,
  },
];

/** Defines the suite for `target` under the title `title`. */
export function dbSuite(title: string, target: () => DbSuiteTarget): void {
  const open = (options: { busyTimeoutMs?: number } = {}) => dbCalls(dbPort(target().adapter(), { ...options, wal: target().wal }));

  describe(title, () => {
    it("creates the database, sets foreign keys and the journal mode, and migrates it", async () => {
      const api = open();
      const { db, version } = ok(await api.open("basics", SCHEMA));
      expect(version).toBe(1);
      expect(target().exists("basics")).toBe(true);
      const journal = ok(await api.query(db, "PRAGMA journal_mode")).rows[0]?.[0];
      if (target().wal) expect(journal).toEqual({ kind: "text", value: "wal" });
      else expect(journal).not.toEqual({ kind: "text", value: "wal" });
      expect(ok(await api.query(db, "PRAGMA foreign_keys")).rows).toEqual([cells(1n)]);
      expect(ok(await api.query(db, "PRAGMA user_version")).rows).toEqual([cells(1n)]);
      ok(await api.close(db));
    });

    it("reports each constraint kind by its extended code", async () => {
      const api = open();
      const { db } = ok(await api.open("constraints", SCHEMA));
      ok(await api.execute(db, "INSERT INTO t (id, name) VALUES (?, ?)", cells(1n, "a")));
      const kind = async (sql: string, params: DbValue[]) => {
        const error = err(await api.execute(db, sql, params));
        expect(error, sql).toBeInstanceOf(DbError.Constraint);
        return (error as DbError.Constraint).kind_;
      };
      expect(await kind("INSERT INTO t (id, name) VALUES (?, ?)", cells(1n, "b")), "PRIMARY KEY").toBe("unique");
      expect(await kind("INSERT INTO t (id, name) VALUES (?, ?)", cells(2n, "a")), "UNIQUE").toBe("unique");
      expect(await kind("INSERT INTO t (id, name) VALUES (?, ?)", cells(2n, null))).toBe("notNull");
      expect(await kind("INSERT INTO t (id, name, parent) VALUES (?, ?, ?)", cells(2n, "c", 99n))).toBe("foreignKey");
      expect(await kind("INSERT INTO t (id, name, n) VALUES (?, ?, ?)", cells(2n, "c", 0n))).toBe("check");
      expect(await kind("INSERT INTO t (id, name) VALUES (?, ?)", cells(2n, "bob")), "a trigger's RAISE").toBe("other");
      const unique = err(await api.execute(db, "INSERT INTO t (id, name) VALUES (?, ?)", cells(3n, "a")));
      expect(unique).toEqual(new DbError.Constraint("unique", "UNIQUE constraint failed: t.name"));
      ok(await api.close(db));
    });

    it("returns changes and the last insert id; SQL errors are Sql", async () => {
      const api = open();
      const { db } = ok(await api.open(":memory:"));
      ok(await api.execute(db, "CREATE TABLE x (id INTEGER PRIMARY KEY, v TEXT)"));
      expect(ok(await api.execute(db, "INSERT INTO x (v) VALUES (?)", cells("a")))).toEqual({ changes: 1n, lastInsertId: 1n });
      expect(ok(await api.execute(db, "INSERT INTO x (v) VALUES (?)", cells("b")))).toEqual({ changes: 1n, lastInsertId: 2n });
      expect(ok(await api.execute(db, "UPDATE x SET v = 'c'"))).toMatchObject({ changes: 2n });
      expect(err(await api.execute(db, "INSERT INTO missing VALUES (1)"))).toEqual(new DbError.Sql("no such table: missing"));
      const syntax = err(await api.query(db, "SELEC 1"));
      expect(syntax).toBeInstanceOf(DbError.Sql);
      expect((syntax as DbError.Sql).message_).toContain("syntax error");
    });

    it("keeps every storage class: i64 extremes as bigint, reals (integral ones too), text, empty and binary blobs, null", async () => {
      const api = open();
      const { db } = ok(await api.open(":memory:"));
      ok(await api.execute(db, "CREATE TABLE c (i INTEGER, j INTEGER, r REAL, s REAL, t TEXT, b BLOB, e BLOB, n TEXT)"));
      const row = cells(-(2n ** 63n), 2n ** 63n - 1n, 1.0, -0.5, "é😀\u0000x", Uint8Array.of(0, 255, 7), new Uint8Array(0), null);
      ok(await api.execute(db, "INSERT INTO c VALUES (?, ?, ?, ?, ?, ?, ?, ?)", row));
      const rows = ok(await api.query(db, "SELECT i, j, r, s, t, b, e, n FROM c"));
      expect(rows.columns).toEqual(["i", "j", "r", "s", "t", "b", "e", "n"]);
      expect(rows.rows).toEqual([row]);
      const types = ok(await api.query(db, "SELECT typeof(i), typeof(r), typeof(t), typeof(b), typeof(e), typeof(n) FROM c"));
      expect(types.rows).toEqual([cells("integer", "real", "text", "blob", "blob", "null")]);
      expect(ok(await api.query(db, "SELECT 1 AS a, 1 AS a")).columns, "duplicate column names stay").toEqual(["a", "a"]);
    });

    it("runs exactly one statement per call, with exactly its parameters", async () => {
      const api = open();
      const { db } = ok(await api.open(":memory:"));
      const several = new DbError.Sql("only one statement per call: use a migration for several");
      expect(err(await api.query(db, "SELECT 1; SELECT 2"))).toEqual(several);
      expect(err(await api.execute(db, "CREATE TABLE a (x); CREATE TABLE b (y)"))).toEqual(several);
      expect(ok(await api.query(db, "SELECT 1;  -- a comment\n /* and another */ ;")).rows).toEqual([cells(1n)]);
      expect(err(await api.query(db, "  -- nothing\n"))).toEqual(new DbError.Sql("the SQL holds no statement"));
      expect(err(await api.query(db, "SELECT ?, ?", cells(1n)))).toEqual(new DbError.Sql("the statement has 2 parameters, 1 were given"));
      expect(err(await api.query(db, "SELECT ?", cells(1n, 2n)))).toEqual(new DbError.Sql("the statement has 1 parameters, 2 were given"));
      expect(ok(await api.query(db, "SELECT ?2, ?1, ?2", cells("a", "b"))).rows).toEqual([cells("b", "a", "b")]);
      expect(ok(await api.query(db, "SELECT :x, ?, :x, @y", cells(1n, 2n, 3n))).rows).toEqual([cells(1n, 2n, 1n, 3n)]);
      const quoted = ok(await api.query(db, "SELECT '?' AS \"a?\", ? AS [b?] /* ? */ -- ?\n", cells(5n)));
      expect(quoted, "a ? in a string, an identifier or a comment is not a parameter").toEqual({ columns: ["a?", "b?"], rows: [cells("?", 5n)] });
    });

    it("a statement on the database during a transaction waits, then fails Busy past the (shortened) timeout", async () => {
      const api = open({ busyTimeoutMs: 100 });
      const { db } = ok(await api.open(":memory:"));
      ok(await api.execute(db, "CREATE TABLE x (v INTEGER)"));
      const tx = ok(await api.begin(db));
      ok(await api.execute(tx, "INSERT INTO x VALUES (1)"));
      expect(err(await api.execute(db, "INSERT INTO x VALUES (2)"))).toEqual(new DbError.Busy());
      ok(await api.rollback(tx));
      expect(ok(await api.query(db, "SELECT COUNT(*) FROM x")).rows, "the rollback undid the insert").toEqual([cells(0n)]);
      const tx2 = ok(await api.begin(db));
      ok(await api.execute(tx2, "INSERT INTO x VALUES (3)"));
      const outside = api.query(db, "SELECT COUNT(*) FROM x");
      ok(await api.commit(tx2));
      expect(ok(await outside).rows, "the waiting statement ran after the commit").toEqual([cells(1n)]);
    });

    it("a garbage file is Corrupt", async () => {
      target().corrupt("garbage");
      expect(err(await open().open("garbage"))).toBeInstanceOf(DbError.Corrupt);
    });

    it("refuses invalid names, unknown ids and a closed database", async () => {
      const api = open();
      expect(err(await api.open("../escape"))).toBeInstanceOf(DbError.Unavailable);
      expect(target().exists("../escape")).toBe(false);
      const { db } = ok(await api.open("closing"));
      ok(await api.close(db));
      ok(await api.close(db));
      expect(err(await api.query(db, "SELECT 1"))).toEqual(new DbError.Unavailable(`no open database or transaction ${db}`));
      expect(err(await api.query(77, "SELECT 1"))).toEqual(new DbError.Unavailable("no open database or transaction 77"));
    });

    it("migrates in one transaction: a failure leaves nothing; a newer database is not downgraded; reopening keeps rows and version", async () => {
      const api = open();
      const broken = [
        { version: 1, sql: "CREATE TABLE a (x INTEGER)" },
        { version: 2, sql: "CREATE TABLE b (y INTEGER); INSERT INTO nowhere VALUES (1)" },
      ];
      const failed = err(await api.open("migrate", broken));
      expect(failed).toBeInstanceOf(DbError.Migration);
      expect((failed as DbError.Migration).version).toBe(2);
      expect((failed as DbError.Migration).message_).toContain("no such table: nowhere");
      const good = [
        { version: 1, sql: "CREATE TABLE a (x INTEGER)" },
        { version: 2, sql: "CREATE TABLE b (y INTEGER); INSERT INTO a VALUES (1)" },
      ];
      const { db, version } = ok(await api.open("migrate", good));
      expect(version, "migration 1 did not survive the failed open").toBe(2);
      ok(await api.close(db));
      expect(err(await api.open("migrate", good.slice(0, 1)))).toEqual(new DbError.Migration(2, "the database is at version 2, newer than the newest migration (1)"));
      const reopened = open();
      const again = ok(await reopened.open("migrate", good));
      expect(again.version).toBe(2);
      expect(ok(await reopened.query(again.db, "SELECT x FROM a")).rows, "a new port over the same storage reads the rows").toEqual([cells(1n)]);
      ok(await reopened.close(again.db));
    });

    it('":memory:" databases are private to their connection', async () => {
      const api = open();
      const a = ok(await api.open(":memory:")).db;
      const b = ok(await api.open(":memory:")).db;
      ok(await api.execute(a, "CREATE TABLE only_in_a (x)"));
      expect(err(await api.query(b, "SELECT * FROM only_in_a"))).toEqual(new DbError.Sql("no such table: only_in_a"));
    });
  });
}
