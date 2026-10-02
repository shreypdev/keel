import { describe, expect, it } from "vitest";
import { DbError, type DbExecuted, type DbRows, type DbValue } from "../src/adapters/types.js";
import { type DbAdapter, type DbConnection, dbPort } from "../src/db.js";
import { dbCalls, err, ok } from "./support/port-calls.js";
import { settle } from "./support/scripted-source.js";

/*
 * The binding of `@undra/runtime/db` (the brief's "Db binding", ADR-048 §4 and §5) over a scripted
 * adapter that records every call: names and versions checked, the open PRAGMAs, migrations in one
 * transaction, the shared id counter, the per-database queue, transactions that make statements on
 * the database wait (and fail Busy after the timeout), unknown and ended ids, close and dispose.
 */

/** A database the scripted adapter keeps: its committed version. */
class ScriptedDb implements DbAdapter {
  readonly versions = new Map<string, number>();
  readonly log: string[] = [];
  /** SQL fragments that fail, with the error they fail with. */
  readonly failures: Array<[string, unknown]> = [];
  /** SQL fragments whose execution waits for a promise (see `gate`). */
  readonly gated = new Map<string, Promise<void>>();
  readonly connections: ScriptedConnection[] = [];
  openFailure: unknown = null;

  async open(name: string): Promise<DbConnection> {
    this.log.push(`open ${name}`);
    if (this.openFailure !== null) throw this.openFailure;
    const connection = new ScriptedConnection(this, name);
    this.connections.push(connection);
    return connection;
  }

  fail(fragment: string, error: unknown): void {
    this.failures.push([fragment, error]);
  }

  /** Makes statements containing `fragment` wait; returns the function that lets them go. */
  gate(fragment: string): () => void {
    let release!: () => void;
    this.gated.set(
      fragment,
      new Promise<void>((resolve) => {
        release = resolve;
      }),
    );
    return release;
  }
}

class ScriptedConnection implements DbConnection {
  pendingVersion: number | null = null;
  closed = false;
  constructor(
    readonly db: ScriptedDb,
    readonly name: string,
  ) {}

  async #run(kind: string, sql: string): Promise<void> {
    this.db.log.push(`${kind} ${sql}`);
    for (const [fragment, opened] of this.db.gated) if (sql.includes(fragment)) await opened;
    for (const [fragment, error] of this.db.failures) if (sql.includes(fragment)) throw error;
  }

  async execute(sql: string, _params: readonly DbValue[]): Promise<DbExecuted> {
    await this.#run("execute", sql);
    const set = /^PRAGMA user_version = (\d+)$/.exec(sql);
    if (set !== null) this.pendingVersion = Number(set[1]);
    if (sql === "COMMIT" && this.pendingVersion !== null) {
      this.db.versions.set(this.name, this.pendingVersion);
      this.pendingVersion = null;
    }
    if (sql === "ROLLBACK") this.pendingVersion = null;
    return { changes: 1n, lastInsertId: 7n };
  }

  async query(sql: string, _params: readonly DbValue[]): Promise<DbRows> {
    await this.#run("query", sql);
    if (sql === "PRAGMA user_version") return { columns: ["user_version"], rows: [[{ kind: "integer", value: BigInt(this.db.versions.get(this.name) ?? 0) }]] };
    return { columns: [], rows: [] };
  }

  async executeScript(sql: string): Promise<void> {
    await this.#run("script", sql);
  }

  async close(): Promise<void> {
    this.closed = true;
    this.db.log.push(`close ${this.name}`);
  }
}

const M = (version: number, sql: string) => ({ version, sql });

describe("dbPort: open", () => {
  it("checks the name before the adapter is asked", async () => {
    const db = new ScriptedDb();
    const api = dbCalls(dbPort(db));
    for (const bad of ["", ".hidden", "a/b", "../escape", "a b", "x".repeat(65), "é"]) {
      const error = err(await api.open(bad));
      expect(error, bad).toBeInstanceOf(DbError.Unavailable);
      expect((error as DbError.Unavailable).value).toContain("invalid database name");
    }
    for (const good of ["app", "a.b_c-1", ":memory:", "x".repeat(64)]) ok(await api.open(good));
    expect(db.log.filter((l) => l.startsWith("open"))).toEqual(["open app", "open a.b_c-1", "open :memory:", `open ${"x".repeat(64)}`]);
  });

  it("checks that migration versions strictly increase from 1", async () => {
    const api = dbCalls(dbPort(new ScriptedDb()));
    expect(err(await api.open("a", [M(0, "x")]))).toEqual(new DbError.Migration(0, "migration versions must strictly increase, starting at 1"));
    expect(err(await api.open("a", [M(2, "x"), M(2, "y")]))).toEqual(new DbError.Migration(2, "migration versions must strictly increase, starting at 1"));
  });

  it("sets the PRAGMAs, runs the pending migrations in one transaction, and answers the version", async () => {
    const db = new ScriptedDb();
    const api = dbCalls(dbPort(db));
    expect(ok(await api.open("app", [M(1, "CREATE a"), M(2, "CREATE b")]))).toEqual({ db: 1, version: 2 });
    expect(db.log).toEqual([
      "open app",
      "query PRAGMA foreign_keys = ON",
      "query PRAGMA busy_timeout = 5000",
      "query PRAGMA journal_mode = WAL",
      "query PRAGMA user_version",
      "execute BEGIN IMMEDIATE",
      "script CREATE a",
      "script CREATE b",
      "execute PRAGMA user_version = 2",
      "execute COMMIT",
    ]);
    db.log.length = 0;
    expect(ok(await api.open("app", [M(1, "CREATE a"), M(2, "CREATE b"), M(3, "CREATE c")]))).toEqual({ db: 2, version: 3 });
    expect(db.log.filter((l) => l.startsWith("script"))).toEqual(["script CREATE c"]);
    db.log.length = 0;
    expect(ok(await api.open("app", [M(1, "CREATE a"), M(2, "CREATE b"), M(3, "CREATE c")]))).toEqual({ db: 3, version: 3 });
    expect(db.log.some((l) => l.includes("BEGIN")), "nothing pending, no transaction").toBe(false);
    expect(ok(await api.open("app"))).toEqual({ db: 4, version: 3 });
  });

  it("leaves WAL out when asked (the web)", async () => {
    const db = new ScriptedDb();
    ok(await dbCalls(dbPort(db, { wal: false })).open("app"));
    expect(db.log.some((l) => l.includes("journal_mode"))).toBe(false);
  });

  it("refuses a database newer than the newest migration, and closes it", async () => {
    const db = new ScriptedDb();
    db.versions.set("app", 3);
    const api = dbCalls(dbPort(db));
    expect(err(await api.open("app", [M(1, "x")]))).toEqual(new DbError.Migration(3, "the database is at version 3, newer than the newest migration (1)"));
    expect(db.connections[0]?.closed).toBe(true);
  });

  it("a failing migration rolls everything back, closes, and is Migration(version, its error's text)", async () => {
    const db = new ScriptedDb();
    db.fail("BROKEN", new DbError.Sql("no such table: nowhere"));
    const api = dbCalls(dbPort(db));
    expect(err(await api.open("app", [M(1, "CREATE a"), M(2, "BROKEN")]))).toEqual(new DbError.Migration(2, "SQL error: no such table: nowhere"));
    expect(db.log.slice(-2)).toEqual(["execute ROLLBACK", "close app"]);
    expect(db.versions.get("app")).toBeUndefined();
  });

  it("an adapter failure that is not a DbError is Unavailable(its text); a typed one passes through", async () => {
    const db = new ScriptedDb();
    const api = dbCalls(dbPort(db));
    db.openFailure = new Error("EACCES: permission denied");
    expect(err(await api.open("app"))).toEqual(new DbError.Unavailable("EACCES: permission denied"));
    db.openFailure = null;
    db.fail("foreign_keys", new DbError.Corrupt("file is not a database"));
    expect(err(await api.open("app"))).toEqual(new DbError.Corrupt("file is not a database"));
    expect(db.connections[0]?.closed, "a database that failed to open is closed").toBe(true);
  });
});

describe("dbPort: statements, transactions, ids", () => {
  it("databases and transactions share one counter from 1; an ended transaction is over", async () => {
    const api = dbCalls(dbPort(new ScriptedDb()));
    expect(ok(await api.open("a")).db).toBe(1);
    expect(ok(await api.begin(1))).toBe(2);
    expect(ok(await api.open("b")).db).toBe(3);
    ok(await api.execute(2, "INSERT 1"));
    ok(await api.commit(2));
    expect(err(await api.execute(2, "INSERT 2"))).toEqual(new DbError.Unavailable("transaction 2 is over"));
    expect(err(await api.commit(2))).toEqual(new DbError.Unavailable("transaction 2 is over"));
    expect(err(await api.query(99, "SELECT 1"))).toEqual(new DbError.Unavailable("no open database or transaction 99"));
    expect(err(await api.begin(1).then(async (tx) => api.begin(ok(tx))))).toEqual(new DbError.Sql("a transaction cannot begin inside a transaction"));
  });

  it("runs a database's operations one at a time, in arrival order", async () => {
    const db = new ScriptedDb();
    const api = dbCalls(dbPort(db));
    ok(await api.open("a"));
    db.log.length = 0;
    const release = db.gate("SLOW");
    const slow = api.execute(1, "SLOW");
    const fast = api.query(1, "FAST");
    await settle();
    expect(db.log, "FAST waits behind SLOW").toEqual(["execute SLOW"]);
    release();
    await Promise.all([slow, fast]);
    expect(db.log).toEqual(["execute SLOW", "query FAST"]);
  });

  it("a statement on the database waits for the running transaction, and runs after it", async () => {
    const db = new ScriptedDb();
    const api = dbCalls(dbPort(db));
    ok(await api.open("a"));
    const tx = ok(await api.begin(1));
    db.log.length = 0;
    const outside = api.execute(1, "OUTSIDE");
    await settle();
    ok(await api.execute(tx, "INSIDE"));
    expect(db.log).toEqual(["execute INSIDE"]);
    ok(await api.commit(tx));
    ok(await outside);
    expect(db.log).toEqual(["execute INSIDE", "execute COMMIT", "execute OUTSIDE"]);
  });

  it("past the busy timeout a waiting statement or begin fails Busy", async () => {
    const api = dbCalls(dbPort(new ScriptedDb(), { busyTimeoutMs: 50 }));
    ok(await api.open("a"));
    const tx = ok(await api.begin(1));
    const started = Date.now();
    expect(err(await api.execute(1, "OUTSIDE"))).toEqual(new DbError.Busy());
    expect(Date.now() - started).toBeGreaterThanOrEqual(45);
    expect(err(await api.begin(1))).toEqual(new DbError.Busy());
    ok(await api.rollback(tx));
    ok(await api.execute(1, "AFTER"));
  });

  it("begin waits for the running transaction, then starts the next", async () => {
    const db = new ScriptedDb();
    const api = dbCalls(dbPort(db));
    ok(await api.open("a"));
    const first = ok(await api.begin(1));
    const second = api.begin(1);
    await settle();
    ok(await api.rollback(first));
    expect(ok(await second)).toBe(first + 1);
    expect(db.log.filter((l) => /BEGIN|ROLLBACK|COMMIT/.test(l))).toEqual(["execute BEGIN IMMEDIATE", "execute ROLLBACK", "execute BEGIN IMMEDIATE"]);
  });

  it("a commit that fails rolls back, ends the transaction and reports the error", async () => {
    const db = new ScriptedDb();
    const api = dbCalls(dbPort(db));
    ok(await api.open("a"));
    const tx = ok(await api.begin(1));
    db.fail("COMMIT", new DbError.Constraint("foreignKey", "FOREIGN KEY constraint failed"));
    expect(err(await api.commit(tx))).toEqual(new DbError.Constraint("foreignKey", "FOREIGN KEY constraint failed"));
    expect(db.log.slice(-2)).toEqual(["execute COMMIT", "execute ROLLBACK"]);
    expect(err(await api.execute(tx, "x"))).toEqual(new DbError.Unavailable(`transaction ${tx} is over`));
    ok(await api.execute(1, "free again"));
  });

  it("close rolls back a running transaction, fails what waits, and is idempotent; afterwards the id is closed", async () => {
    const db = new ScriptedDb();
    const api = dbCalls(dbPort(db));
    ok(await api.open("a"));
    const tx = ok(await api.begin(1));
    const waiting = api.execute(1, "WAITS");
    await settle();
    ok(await api.close(1));
    expect(err(await waiting)).toEqual(new DbError.Unavailable("no open database or transaction 1"));
    expect(db.log.slice(-2)).toEqual(["execute ROLLBACK", "close a"]);
    ok(await api.close(1));
    expect(err(await api.execute(1, "x"))).toEqual(new DbError.Unavailable("no open database or transaction 1"));
    expect(err(await api.execute(tx, "x"))).toEqual(new DbError.Unavailable(`transaction ${tx} is over`));
    expect(err(await api.close(42))).toEqual(new DbError.Unavailable("no open database or transaction 42"));
  });

  it("carries its port's name (what the runtime says about a failure names the Db port)", () => {
    expect(dbPort(new ScriptedDb()).name).toBe("Db");
  });

  it("dispose (the core closed) closes every open database, rolling back what runs", async () => {
    const db = new ScriptedDb();
    const port = dbPort(db);
    const api = dbCalls(port);
    ok(await api.open("a"));
    ok(await api.open("b"));
    ok(await api.begin(2));
    port.dispose?.();
    await settle();
    expect(db.connections.map((c) => c.closed)).toEqual([true, true]);
    expect(db.log.slice(-3)).toEqual(["close a", "execute ROLLBACK", "close b"]);
  });
});
