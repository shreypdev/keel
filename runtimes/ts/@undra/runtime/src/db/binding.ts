import { DbErrorCodec, DbExecutedCodec, DbMigrationCodec, DbOpenedCodec, DbRowsCodec, DbValueCodec } from "../adapters/codecs.js";
import { PortIds } from "../adapters/ids.js";
import { DbError, type DbExecuted, type DbMigration, type DbRows, type DbValue } from "../adapters/types.js";
import { UndraPortError } from "../errors.js";
import { errorMessage } from "../platform.js";
import type { PortImpl } from "../port.js";
import { UndraReader, codecs, encodeValue } from "../wire/index.js";

/**
 * SQLite on the platform, for the core's `Db` port (ADR-048): opens a database by name. Implement
 * it to replace the default ({@link nodeSqliteDb} on Node); {@link dbPort} turns it into the port
 * and owns ids, migrations, transactions and the per-database queue, so an adapter only runs SQL.
 */
export interface DbAdapter {
  /**
   * Opens (creating it if needed) database `name`: `":memory:"` for a private in-memory one, else a
   * file named after it in the adapter's directory. `name` was checked by the binding. Rejects with
   * a {@link DbError} (`Unavailable` for a file that cannot be opened).
   */
  open(name: string): Promise<DbConnection>;
}

/**
 * One open database of a {@link DbAdapter}. The binding calls it one operation at a time, in order;
 * every method rejects with a {@link DbError} chosen by SQLite's result code (ADR-048 §2).
 */
export interface DbConnection {
  /**
   * Runs exactly one statement with `params` bound positionally (`?`, `?NNN`); trailing SQL is
   * `Sql("only one statement per call: use a migration for several")` and a parameter count
   * that differs from the statement's is `Sql("the statement has N parameters, M were given")`.
   */
  execute(sql: string, params: readonly DbValue[]): Promise<DbExecuted>;
  /** Runs exactly one query (same rules as `execute`) and returns its column names and every row, each cell by its storage class. */
  query(sql: string, params: readonly DbValue[]): Promise<DbRows>;
  /** Runs `sql`, which may hold several statements (a migration). */
  executeScript(sql: string): Promise<void>;
  /** Closes the database. Never rejects. */
  close(): Promise<void>;
}

/** Options of {@link dbPort}. */
export interface DbPortOptions {
  /** How long a statement on a database waits for a running transaction to end before it fails `Busy`, in ms. Default 5,000 (tests shorten it). */
  readonly busyTimeoutMs?: number;
  /** Whether `open` sets `journal_mode = WAL`. Default `true`; `false` where the VFS has no WAL (the web). */
  readonly wal?: boolean;
}

/** The longest database name (ADR-048 §6). */
export const MAX_DB_NAME_LENGTH = 64;

/** Checks a database name: `":memory:"`, or 1 to 64 of `A-Z a-z 0-9 . _ -` not starting with `.`. */
export function validateDbName(name: string): void {
  const valid = name === ":memory:" || (/^[A-Za-z0-9._-]{1,64}$/.test(name) && !name.startsWith("."));
  if (!valid) {
    throw new DbError.Unavailable(
      `invalid database name ${JSON.stringify(name)}: use 1 to 64 of A-Z a-z 0-9 . _ - (not starting with .), or ":memory:"`,
    );
  }
}

/** Checks that migration versions strictly increase from 1. */
export function validateMigrations(migrations: readonly DbMigration[]): void {
  let last = 0;
  for (const { version } of migrations) {
    if (version <= last) throw new DbError.Migration(version, "migration versions must strictly increase, starting at 1");
    last = version;
  }
}

/** What the binding keeps per open database. */
interface Database {
  readonly id: number;
  readonly conn: DbConnection;
  closed: boolean;
  /** The running transaction's id. */
  tx: number | null;
  /** Statements and `begin`s waiting for the transaction to end. */
  readonly waiters: Set<() => void>;
  /** The end of the serial queue. */
  tail: Promise<void>;
}

interface Transaction {
  readonly id: number;
  readonly db: Database;
}

const migrations = /* @__PURE__ */ codecs.vec(DbMigrationCodec);
const values = /* @__PURE__ */ codecs.vec(DbValueCodec);
const EMPTY = new Uint8Array(0);

function readArgs<T>(args: Uint8Array, read: (r: UndraReader) => T): T {
  const r = new UndraReader(args);
  const value = read(r);
  r.finish();
  return value;
}

/** `error` as a {@link DbError}: a typed one as it is, anything else as `Unavailable(<its text>)`. */
function asDbError(error: unknown): DbError {
  return error instanceof DbError ? error : new DbError.Unavailable(errorMessage(error));
}

async function typed(work: () => Promise<Uint8Array>): Promise<Uint8Array> {
  try {
    return await work();
  } catch (error) {
    throw new UndraPortError(encodeValue(DbErrorCodec, asDbError(error)));
  }
}

async function quietly(work: () => Promise<unknown>): Promise<void> {
  try {
    await work();
  } catch {
    // A cleanup step (a ROLLBACK after a failure, a close) has nobody to tell.
  }
}

/** The integer in the first cell of `rows` (`PRAGMA user_version`). */
function firstInteger(rows: DbRows): number {
  const cell = rows.rows[0]?.[0];
  if (cell?.kind === "integer") return Number(cell.value);
  if (cell?.kind === "real") return cell.value;
  return 0;
}

/**
 * The core's `Db` port over `adapter` (ADR-048), for `LoadOptions.ports` or
 * `core.registerPort(PortIds.Db.portId, ...)`. One instance per core. The binding implements the
 * ADR's semantics; the adapter only runs SQL.
 *
 * * Ids: databases and transactions share one counter from 1, never reused.
 * * `open(name, migrations)` checks the name and the versions, opens the connection, sets
 *   `foreign_keys = ON`, `busy_timeout = 5000` and (unless `wal: false`) `journal_mode = WAL`, refuses a
 *   database newer than the newest migration, and runs the pending migrations in one `BEGIN
 *   IMMEDIATE` transaction (any failure rolls everything back: `Migration { version, message }`).
 * * One serial queue per database. `begin` waits until no transaction runs (at most
 *   `busyTimeoutMs`, then `Busy`) and starts one; statements on a transaction id run in the queue
 *   at once; statements on the database id wait for the transaction to end (the same timeout).
 *   `commit` (`ROLLBACK` when it fails) and `rollback` end it. An unknown or closed id is
 *   `Unavailable("no open database or transaction <id>")`, an ended transaction
 *   `Unavailable("transaction <id> is over")`.
 * * `close` rolls back a running transaction and closes the connection; closing again is not an
 *   error. When the core closes, the port is replaced, or a wasm core restarts after a trap
 *   (`dispose`), every database is closed and its running transaction rolled back; after a restart
 *   the port serves the new instance.
 */
export function dbPort(adapter: DbAdapter, options: DbPortOptions = {}): PortImpl {
  const busyTimeoutMs = Math.max(0, options.busyTimeoutMs ?? 5000);
  const wal = options.wal ?? true;
  const databases = new Map<number, Database>();
  const transactions = new Map<number, Transaction>();
  let next = 1;
  /** Bumped by `dispose`: an open that was under way then closes its connection instead of registering it. */
  let epoch = 0;

  /** Runs `op` on `db`'s queue, after everything queued before it. */
  const enqueue = <T>(db: Database, op: () => Promise<T>): Promise<T> => {
    const run = db.tail.then(op);
    db.tail = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  };

  const noSuch = (id: number): DbError => new DbError.Unavailable(`no open database or transaction ${id}`);

  /** The database or transaction `id` names. */
  const target = (id: number): { readonly db: Database; readonly tx: Transaction | null } => {
    const tx = transactions.get(id);
    if (tx !== undefined) return { db: tx.db, tx };
    const db = databases.get(id);
    if (db !== undefined && !db.closed) return { db, tx: null };
    if (db === undefined && id > 0 && id < next) throw new DbError.Unavailable(`transaction ${id} is over`);
    throw noSuch(id);
  };

  /** Wakes everything that waits for `db`'s transaction to end. */
  const wake = (db: Database): void => {
    for (const waiter of [...db.waiters]) waiter();
  };

  /**
   * Calls `claim` (synchronously, so nothing can start a transaction in between) once `db` has no
   * transaction; waits at most the busy timeout for that.
   */
  const whenIdle = async <T>(db: Database, claim: () => Promise<T>): Promise<T> => {
    const deadline = Date.now() + busyTimeoutMs;
    for (;;) {
      if (db.closed) throw noSuch(db.id);
      if (db.tx === null) return claim();
      const remaining = deadline - Date.now();
      if (remaining <= 0) throw new DbError.Busy();
      await new Promise<void>((resolve) => {
        const done = (): void => {
          clearTimeout(timer);
          db.waiters.delete(done);
          resolve();
        };
        const timer = setTimeout(done, remaining);
        db.waiters.add(done);
      });
    }
  };

  /** Ends `tx`: its id is over at once; `db`'s statements are released when `finish` runs. */
  const endTransaction = (tx: Transaction): (() => void) => {
    transactions.delete(tx.id);
    return () => {
      if (tx.db.tx === tx.id) tx.db.tx = null;
      wake(tx.db);
    };
  };

  /** Runs one statement on database or transaction `id`. */
  const statement = <T>(id: number, run: (conn: DbConnection) => Promise<T>): Promise<T> => {
    const { db, tx } = target(id);
    if (tx !== null) return enqueue(db, () => run(db.conn));
    return whenIdle(db, () => enqueue(db, () => run(db.conn)));
  };

  const closeDatabase = (db: Database): Promise<void> => {
    db.closed = true;
    const running = db.tx === null ? undefined : transactions.get(db.tx);
    if (running !== undefined) {
      const finish = endTransaction(running);
      void enqueue(db, () => quietly(() => db.conn.execute("ROLLBACK", []))).then(finish);
    }
    wake(db);
    return enqueue(db, () => quietly(() => db.conn.close()));
  };

  const open = async (name: string, wanted: readonly DbMigration[]): Promise<{ db: number; version: number }> => {
    validateDbName(name);
    validateMigrations(wanted);
    const asked = epoch;
    const conn = await adapter.open(name);
    try {
      await conn.query("PRAGMA foreign_keys = ON", []);
      await conn.query("PRAGMA busy_timeout = 5000", []);
      if (wal) await conn.query("PRAGMA journal_mode = WAL", []);
      const current = firstInteger(await conn.query("PRAGMA user_version", []));
      const newest = wanted.at(-1)?.version ?? 0;
      if (wanted.length > 0 && current > newest) {
        throw new DbError.Migration(current, `the database is at version ${current}, newer than the newest migration (${newest})`);
      }
      const pending = wanted.filter((m) => m.version > current);
      let version = current;
      const last = pending.at(-1);
      if (last !== undefined) {
        await conn.execute("BEGIN IMMEDIATE", []);
        let at = last.version;
        try {
          for (const migration of pending) {
            at = migration.version;
            await conn.executeScript(migration.sql);
          }
          at = last.version;
          await conn.execute(`PRAGMA user_version = ${last.version}`, []);
          await conn.execute("COMMIT", []);
        } catch (error) {
          await quietly(() => conn.execute("ROLLBACK", []));
          throw new DbError.Migration(at, errorMessage(error));
        }
        version = last.version;
      }
      if (epoch !== asked) throw new DbError.Unavailable("the core went away while the database opened");
      const id = next++;
      databases.set(id, { id, conn, closed: false, tx: null, waiters: new Set(), tail: Promise.resolve() });
      return { db: id, version };
    } catch (error) {
      await quietly(() => conn.close());
      throw error;
    }
  };

  const ids = PortIds.Db;
  return {
    name: "Db",
    sync: false,
    methods: {
      [ids.open]: (args) => {
        const [name, wanted] = readArgs(args, (r) => [r.readStr(), migrations.decode(r)] as const);
        return typed(async () => encodeValue(DbOpenedCodec, await open(name, wanted)));
      },
      [ids.execute]: (args) => {
        const [id, sql, params] = readArgs(args, (r) => [r.readU32(), r.readStr(), values.decode(r)] as const);
        return typed(async () => encodeValue(DbExecutedCodec, await statement(id, (conn) => conn.execute(sql, params))));
      },
      [ids.query]: (args) => {
        const [id, sql, params] = readArgs(args, (r) => [r.readU32(), r.readStr(), values.decode(r)] as const);
        return typed(async () => encodeValue(DbRowsCodec, await statement(id, (conn) => conn.query(sql, params))));
      },
      [ids.begin]: (args) => {
        const id = readArgs(args, (r) => r.readU32());
        return typed(async () => {
          const { db, tx } = target(id);
          if (tx !== null) throw new DbError.Sql("a transaction cannot begin inside a transaction");
          const started = await whenIdle(db, () => {
            const begun: Transaction = { id: next++, db };
            transactions.set(begun.id, begun);
            db.tx = begun.id;
            return enqueue(db, () => db.conn.execute("BEGIN IMMEDIATE", [])).then(
              () => begun.id,
              (error: unknown) => {
                endTransaction(begun)();
                throw error;
              },
            );
          });
          return encodeValue(codecs.u32, started);
        });
      },
      [ids.commit]: (args) => {
        const id = readArgs(args, (r) => r.readU32());
        return typed(async () => {
          const { db, tx } = target(id);
          if (tx === null) throw noSuch(id);
          const finish = endTransaction(tx);
          try {
            await enqueue(db, () => db.conn.execute("COMMIT", []));
          } catch (error) {
            await enqueue(db, () => quietly(() => db.conn.execute("ROLLBACK", [])));
            throw error;
          } finally {
            finish();
          }
          return EMPTY;
        });
      },
      [ids.rollback]: (args) => {
        const id = readArgs(args, (r) => r.readU32());
        return typed(async () => {
          const { db, tx } = target(id);
          if (tx === null) throw noSuch(id);
          const finish = endTransaction(tx);
          try {
            await enqueue(db, () => db.conn.execute("ROLLBACK", []));
          } finally {
            finish();
          }
          return EMPTY;
        });
      },
      [ids.close]: (args) => {
        const id = readArgs(args, (r) => r.readU32());
        return typed(async () => {
          const db = databases.get(id);
          if (db === undefined) throw noSuch(id);
          if (!db.closed) await closeDatabase(db);
          return EMPTY;
        });
      },
    },
    dispose() {
      epoch++;
      for (const db of databases.values()) if (!db.closed) void closeDatabase(db);
    },
  };
}
