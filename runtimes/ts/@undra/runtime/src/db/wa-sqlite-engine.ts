import { DbError, type DbExecuted, type DbRows, type DbValue } from "../adapters/types.js";
import { errorMessage } from "../platform.js";
import type { DbAdapter, DbConnection } from "./binding.js";
import { sqliteError } from "./errors.js";

/*
 * The Db adapter over wa-sqlite's API (ADR-048 §7), written against the part of `SQLiteAPI` it uses
 * so this file imports nothing from wa-sqlite: the worker entry (`@undra/runtime/db-worker`) hands
 * it the API it built, and so do the tests, with wa-sqlite's in-memory VFS on Node. Internal to
 * `@undra/runtime/db` and `@undra/runtime/db-worker`.
 */

/** The part of wa-sqlite's `SQLiteAPI` (`Factory(module)`) the engine uses. */
export interface SqliteApi {
  open_v2(filename: string, flags: number, vfs?: string): Promise<number>;
  close(db: number): Promise<number>;
  exec(db: number, sql: string): Promise<number>;
  str_new(db: number, s?: string): number;
  str_value(str: number): number;
  str_finish(str: number): void;
  prepare_v2(db: number, sql: number): Promise<{ readonly stmt: number; readonly sql: number } | null>;
  finalize(stmt: number): Promise<number>;
  bind_parameter_count(stmt: number): number;
  bind_null(stmt: number, i: number): number;
  bind_int64(stmt: number, i: number, value: bigint): number;
  bind_double(stmt: number, i: number, value: number): number;
  step(stmt: number): Promise<number>;
  column_count(stmt: number): number;
  column_names(stmt: number): string[];
  column_type(stmt: number, i: number): number;
  column_int64(stmt: number, i: number): bigint;
  column_double(stmt: number, i: number): number;
  column_blob(stmt: number, i: number): Uint8Array;
}

/**
 * The part of wa-sqlite's Emscripten module the engine uses directly: its `bind_text` binds a
 * NUL-terminated copy (text holding U+0000 would be cut) and its `bind_blob` binds a NULL pointer
 * for an empty array (SQLite stores NULL), so text and blobs are bound here with their length.
 */
export interface SqliteModule {
  readonly HEAPU8: Uint8Array;
  _sqlite3_malloc(bytes: number): number;
  _sqlite3_bind_text(stmt: number, i: number, data: number, bytes: number, destructor: number): number;
  _sqlite3_bind_blob(stmt: number, i: number, data: number, bytes: number, destructor: number): number;
  _getSqliteFree(): number;
}

const SQLITE_OPEN_READWRITE = 0x2;
const SQLITE_OPEN_CREATE = 0x4;
/** Extended result codes from every call (SQLite 3.37+): the constraint kind needs them, and wa-sqlite exports no `sqlite3_extended_errcode`. */
const SQLITE_OPEN_EXRESCODE = 0x0200_0000;
const SQLITE_ROW = 100;
const SQLITE_RANGE = 25;

const utf8 = new TextEncoder();
const fromUtf8 = new TextDecoder();

const INTEGER = 1;
const FLOAT = 2;
const TEXT = 3;
const BLOB = 4;

/** What wa-sqlite threw, as a {@link DbError}: by its result `code` (extended, see the open flags), else `Sql`. */
function translate(error: unknown): DbError {
  if (error instanceof DbError) return error;
  const code = (error as { readonly code?: unknown } | null)?.code;
  if (typeof code === "number") return sqliteError(code, errorMessage(error));
  return new DbError.Sql(errorMessage(error));
}

/** Runs `work`, turning what wa-sqlite throws into a {@link DbError}. */
async function guarded<T>(work: () => Promise<T>): Promise<T> {
  try {
    return await work();
  } catch (error) {
    throw translate(error);
  }
}

/** Binds `bytes` (text or a blob) to parameter `at`, copied into SQLite's memory with its length. */
function bindBytes(module: SqliteModule, stmt: number, at: number, bytes: Uint8Array, kind: "text" | "blob"): number {
  // At least one byte, so an empty value has a pointer: a NULL pointer would bind SQL NULL.
  const data = module._sqlite3_malloc(Math.max(1, bytes.length));
  if (data === 0) return 7; // SQLITE_NOMEM
  module.HEAPU8.set(bytes, data);
  const free = module._getSqliteFree();
  // SQLite owns `data` from here, and frees it with `free` even when binding fails.
  return kind === "text" ? module._sqlite3_bind_text(stmt, at, data, bytes.length, free) : module._sqlite3_bind_blob(stmt, at, data, bytes.length, free);
}

/** One open database. */
class WaSqliteConnection implements DbConnection {
  readonly #api: SqliteApi;
  readonly #module: SqliteModule;
  readonly #db: number;
  #closed = false;

  constructor(api: SqliteApi, module: SqliteModule, db: number) {
    this.#api = api;
    this.#module = module;
    this.#db = db;
  }

  /**
   * Prepares exactly one statement of `sql` (SQLite's own parser finds the end: a second statement
   * in the tail is refused, whitespace, comments and semicolons are not), binds `params`
   * positionally, runs `use` on it and finalizes it.
   */
  async #statement<T>(sql: string, params: readonly DbValue[], use: (stmt: number) => Promise<T>): Promise<T> {
    if (this.#closed) throw new DbError.Unavailable("the database is closed");
    const api = this.#api;
    const text = api.str_new(this.#db, sql);
    let stmt = 0;
    try {
      const prepared = await api.prepare_v2(this.#db, api.str_value(text));
      if (prepared === null) throw new DbError.Sql("the SQL holds no statement");
      stmt = prepared.stmt;
      let rest: { readonly stmt: number } | null;
      try {
        rest = await api.prepare_v2(this.#db, prepared.sql);
      } catch {
        rest = { stmt: 0 };
      }
      if (rest !== null) {
        if (rest.stmt !== 0) await api.finalize(rest.stmt);
        throw new DbError.Sql("only one statement per call: use a migration for several");
      }
      const count = api.bind_parameter_count(stmt);
      if (params.length !== count) throw new DbError.Sql(`the statement has ${count} parameters, ${params.length} were given`);
      params.forEach((value, index) => {
        const at = index + 1;
        let rc: number;
        switch (value.kind) {
          case "null":
            rc = api.bind_null(stmt, at);
            break;
          case "integer":
            rc = api.bind_int64(stmt, at, value.value);
            break;
          case "real":
            rc = api.bind_double(stmt, at, value.value);
            break;
          case "text":
            rc = bindBytes(this.#module, stmt, at, utf8.encode(value.value), "text");
            break;
          case "blob":
            rc = bindBytes(this.#module, stmt, at, value.value, "blob");
            break;
        }
        if (rc === SQLITE_RANGE) throw sqliteError(rc, `parameter ${at} is out of range`);
        if (rc !== 0) throw sqliteError(rc, `parameter ${at} could not be bound`);
      });
      return await use(stmt);
    } catch (error) {
      throw translate(error);
    } finally {
      if (stmt !== 0) await api.finalize(stmt).catch(() => {});
      api.str_finish(text);
    }
  }

  /** The cell `i` of the current row, by its storage class. */
  #cell(stmt: number, i: number): DbValue {
    const api = this.#api;
    switch (api.column_type(stmt, i)) {
      case INTEGER:
        return { kind: "integer", value: api.column_int64(stmt, i) };
      case FLOAT:
        return { kind: "real", value: api.column_double(stmt, i) };
      case TEXT:
        // As bytes and their length: `column_text` reads up to the first U+0000.
        return { kind: "text", value: fromUtf8.decode(api.column_blob(stmt, i)) };
      case BLOB:
        // A view into the module's memory: copy it before the next step reuses it.
        return { kind: "blob", value: api.column_blob(stmt, i).slice() };
      default:
        return { kind: "null" };
    }
  }

  async execute(sql: string, params: readonly DbValue[]): Promise<DbExecuted> {
    await this.#statement(sql, params, async (stmt) => {
      while ((await this.#api.step(stmt)) === SQLITE_ROW) {
        // Rows of a statement run for its effect are not read.
      }
    });
    // `changes()` and `last_insert_rowid()` as SQL: wa-sqlite exports neither 64-bit C function.
    const [counts] = (await this.query("SELECT changes(), last_insert_rowid()", [])).rows;
    const changes = counts?.[0];
    const last = counts?.[1];
    return {
      changes: changes?.kind === "integer" ? changes.value : 0n,
      lastInsertId: last?.kind === "integer" ? last.value : 0n,
    };
  }

  query(sql: string, params: readonly DbValue[]): Promise<DbRows> {
    return this.#statement(sql, params, async (stmt) => {
      const columns = this.#api.column_names(stmt);
      const rows: DbValue[][] = [];
      while ((await this.#api.step(stmt)) === SQLITE_ROW) {
        const row: DbValue[] = [];
        for (let i = 0; i < columns.length; i++) row.push(this.#cell(stmt, i));
        rows.push(row);
      }
      return { columns, rows };
    });
  }

  async executeScript(sql: string): Promise<void> {
    if (this.#closed) throw new DbError.Unavailable("the database is closed");
    await guarded(() => this.#api.exec(this.#db, sql));
  }

  async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    try {
      await this.#api.close(this.#db);
    } catch {
      // Closing fails only for a handle that is already gone.
    }
  }
}

/**
 * A {@link DbAdapter} over wa-sqlite's `api` (built from `module`), opening databases on the VFS registered as `vfs`
 * at `pathOf(name)` (`":memory:"` stays in memory). `prepare`, when given, runs before every open
 * (the OPFS pool grows its capacity there).
 */
export function waSqliteEngine(
  api: SqliteApi,
  module: SqliteModule,
  vfs: string,
  pathOf: (name: string) => string,
  prepare?: () => Promise<void>,
): DbAdapter {
  return {
    async open(name) {
      await prepare?.();
      const path = name === ":memory:" ? ":memory:" : pathOf(name);
      let db: number;
      try {
        db = await api.open_v2(path, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_EXRESCODE, vfs);
      } catch (error) {
        const failure = translate(error);
        throw failure instanceof DbError.Sql ? new DbError.Unavailable(failure.message_) : failure;
      }
      return new WaSqliteConnection(api, module, db);
    },
  };
}
