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

/** The first index at which two byte arrays differ (the shorter one's length when it is a prefix of the other), or -1 when they are the same. */
function firstDifference(a: Uint8Array, b: Uint8Array): number {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) if (a[i] !== b[i]) return i;
  return a.length === b.length ? -1 : n;
}

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

    it("a statement on the database during a transaction fails Busy past the (shortened) timeout", async () => {
      const api = open({ busyTimeoutMs: 100 });
      const { db } = ok(await api.open(":memory:"));
      ok(await api.execute(db, "CREATE TABLE x (v INTEGER)"));
      const tx = ok(await api.begin(db));
      ok(await api.execute(tx, "INSERT INTO x VALUES (1)"));
      expect(err(await api.execute(db, "INSERT INTO x VALUES (2)"))).toEqual(new DbError.Busy());
      ok(await api.rollback(tx));
      expect(ok(await api.query(db, "SELECT COUNT(*) FROM x")).rows, "the rollback undid the insert").toEqual([cells(0n)]);
    });

    // A busy timeout that never elapses here (60 s: a hang detector, not a speed): with the 100 ms of the test above, a machine that took that long between this
    // statement and the commit made it Busy, and the test failed for the machine's speed.
    it("a statement on the database during a transaction waits for it, however long that takes, and runs after the commit", async () => {
      const api = open({ busyTimeoutMs: 60_000 });
      const { db } = ok(await api.open(":memory:"));
      ok(await api.execute(db, "CREATE TABLE x (v INTEGER)"));
      const tx2 = ok(await api.begin(db));
      ok(await api.execute(tx2, "INSERT INTO x VALUES (3)"));
      const outside = api.query(db, "SELECT COUNT(*) FROM x");
      ok(await api.commit(tx2));
      expect(ok(await outside).rows, "the waiting statement ran after the commit").toEqual([cells(1n)]);
    });

    it("an outer statement waits at most the busy timeout, while the transaction's own statements go on", async () => {
      const api = open({ busyTimeoutMs: 200 });
      const { db } = ok(await api.open(":memory:"));
      ok(await api.execute(db, "CREATE TABLE x (v INTEGER)"));
      const tx = ok(await api.begin(db));
      const started = Date.now();
      // When the outer statement settled, and whether a guard timer for one and a half times the timeout had fired by then. Both are taken as it
      // settles, not after the transaction's statements below, which a slow machine can make take longer than either bound.
      let guardFired = false;
      let settled: { waited: number; late: boolean } | undefined;
      const outside = api.execute(db, "INSERT INTO x VALUES (0)").then((outcome) => {
        settled = { waited: Date.now() - started, late: guardFired };
        return outcome;
      });
      // The guard is armed after the call, which armed the statement's own timer of the busy timeout synchronously: its deadline is the later one on
      // the same clock. Overdue timers fire in the order of their deadlines, with the microtasks of each run before the next, so on any machine the
      // Busy that the statement's timer produces settles before the guard fires; one that settles after it is late by the code's doing.
      const guard = setTimeout(() => {
        guardFired = true;
      }, 1.5 * 200);
      for (let i = 1n; i <= 5n; i++) ok(await api.execute(tx, "INSERT INTO x VALUES (?)", cells(i)));
      expect(err(await outside)).toEqual(new DbError.Busy());
      clearTimeout(guard);
      expect(settled?.waited, "not before the timeout").toBeGreaterThanOrEqual(190);
      expect(settled?.late, "and not long after it: Busy came before a timer for one and a half times the timeout, armed after the statement's").toBe(false);
      ok(await api.commit(tx));
      expect(ok(await api.query(db, "SELECT v FROM x ORDER BY v")).rows, "the outer insert never ran").toEqual([1n, 2n, 3n, 4n, 5n].map((v) => cells(v)));
    });

    // The busy timeout here is one that never elapses (60 s, so a hang detector and not a speed): the outer statement is still waiting when the transaction's
    // five statements are done, on any machine. Queued behind it they would wait for it, and it for them, and the test would hang to its timeout.
    it("the transaction's own statements are not queued behind an outer statement that waits for the transaction", async () => {
      const api = open({ busyTimeoutMs: 60_000 });
      const { db } = ok(await api.open(":memory:"));
      ok(await api.execute(db, "CREATE TABLE x (v INTEGER)"));
      const tx = ok(await api.begin(db));
      let settled = false;
      const outside = api.execute(db, "INSERT INTO x VALUES (0)").finally(() => {
        settled = true;
      });
      for (let i = 1n; i <= 5n; i++) {
        ok(await api.execute(tx, "INSERT INTO x VALUES (?)", cells(i)));
        expect(settled, `statement ${i} of the transaction ran while the outer one was still waiting`).toBe(false);
      }
      ok(await api.commit(tx));
      expect(ok(await outside), "the outer statement ran once the transaction ended").toMatchObject({ changes: 1n });
      expect(ok(await api.query(db, "SELECT v FROM x ORDER BY v")).rows).toEqual([0n, 1n, 2n, 3n, 4n, 5n].map((v) => cells(v)));
    }, 30_000);

    it("binds values, never splices them: SQL in a text parameter is data", async () => {
      const api = open();
      const { db } = ok(await api.open(":memory:"));
      ok(await api.execute(db, "CREATE TABLE t (name TEXT)"));
      const evil = "'; DROP TABLE t; --";
      ok(await api.execute(db, "INSERT INTO t (name) VALUES (?)", cells(evil)));
      expect(ok(await api.query(db, "SELECT name FROM t WHERE name = ?", cells(evil))).rows).toEqual([cells(evil)]);
      expect(ok(await api.query(db, "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?", cells("t"))).rows, "t is still there").toEqual([cells(1n)]);
    });

    // `toEqual` over a million bytes took most of the test's second (and prints a million lines when it fails); this is the same byte-for-byte check. The timeout is a hang
    // detector, generous because a throttled machine took more than the default 5 s: it says nothing about speed.
    it("carries a 2 MB row whole, and integers past 2^53 exactly", async () => {
      const api = open();
      const { db } = ok(await api.open(":memory:"));
      ok(await api.execute(db, "CREATE TABLE big (t TEXT, b BLOB, i INTEGER, j INTEGER)"));
      const text = `${"é😀".repeat(170_000)}\u0000end`;
      const blob = Uint8Array.from({ length: 1 << 20 }, (_, i) => (i * 31) & 0xff);
      const row = cells(text, blob, 2n ** 53n + 1n, -(2n ** 53n) - 3n);
      ok(await api.execute(db, "INSERT INTO big VALUES (?, ?, ?, ?)", row));
      expect(ok(await api.query(db, "SELECT length(CAST(t AS BLOB)) + length(b) FROM big")).rows[0]?.[0]).toEqual({ kind: "integer", value: BigInt(new TextEncoder().encode(text).length + blob.length) });
      const rows = ok(await api.query(db, "SELECT t, b, i, j FROM big")).rows;
      expect(rows).toHaveLength(1);
      const [t, b, i, j] = rows[0] as DbValue[];
      expect(t?.kind === "text" && t.value === text, "the text, U+0000 and all").toBe(true);
      expect(b?.kind).toBe("blob");
      expect(b?.kind === "blob" ? firstDifference(b.value, blob) : -2, "the blob, byte for byte (-1: the same; otherwise the first index that differs)").toBe(-1);
      expect([i, j]).toEqual(cells(2n ** 53n + 1n, -(2n ** 53n) - 3n));
    }, 60_000);

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

    it("a failed migration of a database that holds data leaves it as it was: version, schema and rows", async () => {
      const api = open();
      const v1 = [{ version: 1, sql: "CREATE TABLE a (x INTEGER); INSERT INTO a VALUES (7)" }];
      ok(await api.close(ok(await api.open("upgrade", v1)).db));
      const v2 = [...v1, { version: 2, sql: "ALTER TABLE a ADD COLUMN y INTEGER; CREATE TABLE b (z); UPDATE a SET x = 8; INSERT INTO nowhere VALUES (1)" }];
      const failed = err(await api.open("upgrade", v2));
      expect(failed).toBeInstanceOf(DbError.Migration);
      expect((failed as DbError.Migration).version).toBe(2);
      const again = ok(await api.open("upgrade", v1));
      expect(again.version).toBe(1);
      expect(ok(await api.query(again.db, "PRAGMA user_version")).rows).toEqual([cells(1n)]);
      expect(ok(await api.query(again.db, "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")).rows, "no table b").toEqual([cells("a")]);
      expect(ok(await api.query(again.db, "SELECT * FROM a")), "no column y, the row unchanged").toEqual({ columns: ["x"], rows: [cells(7n)] });
      ok(await api.close(again.db));
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
