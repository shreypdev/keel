import { DbError, type DbExecuted, type DbRows, type DbValue } from "../adapters/types.js";
import { errorMessage } from "../platform.js";
import type { DbAdapter, DbConnection } from "./binding.js";
import { nodeBuiltin } from "../node-builtin.js";
import { sqliteError } from "./errors.js";
import { isTrivia, scanParameters } from "./sql.js";

/*
 * `nodeSqliteDb()`: the Db adapter of Node, over the built-in `node:sqlite` (`DatabaseSync`, Node
 * 22.5+). Node's modules are reached on first use through `process.getBuiltinModule`, which no
 * bundler sees, and the parts used are declared here, so the runtime compiles without Node's types.
 */

/** A value `node:sqlite` binds or reads. */
type NodeSqlValue = null | bigint | number | string | Uint8Array;

/** The parts of `node:sqlite`'s `StatementSync` the adapter uses. */
interface NodeStatement {
  readonly sourceSQL: string;
  setReadBigInts(enabled: boolean): void;
  setReturnArrays(enabled: boolean): void;
  columns(): Array<{ readonly name: string }>;
  run(...params: unknown[]): { readonly changes: number | bigint; readonly lastInsertRowid: number | bigint };
  all(...params: unknown[]): NodeSqlValue[][];
}

/** The parts of `node:sqlite`'s `DatabaseSync` the adapter uses. */
interface NodeDatabase {
  prepare(sql: string): NodeStatement;
  exec(sql: string): void;
  close(): void;
}

interface NodeSqliteModule {
  readonly DatabaseSync: new (path: string) => NodeDatabase;
}

interface NodeFsModule {
  mkdir(path: string, options: { readonly recursive: true }): Promise<unknown>;
}

/** What `node:sqlite` throws for an SQLite failure: the extended result code in `errcode`. */
interface NodeSqliteFailure {
  readonly errcode?: unknown;
  readonly message?: unknown;
}

/** What `node:sqlite` threw, as a {@link DbError}: by `errcode` when it has one, else `Sql`. */
function translate(error: unknown): DbError {
  if (error instanceof DbError) return error;
  const failure = error as NodeSqliteFailure | null;
  const message = errorMessage(error);
  if (typeof failure?.errcode === "number") return sqliteError(failure.errcode, message);
  return new DbError.Sql(message);
}

function toNode(value: DbValue): NodeSqlValue {
  return value.kind === "null" ? null : value.value;
}

function fromNode(value: unknown): DbValue {
  if (value === null || value === undefined) return { kind: "null" };
  if (typeof value === "bigint") return { kind: "integer", value };
  if (typeof value === "number") return { kind: "real", value };
  if (typeof value === "string") return { kind: "text", value };
  if (value instanceof Uint8Array) return { kind: "blob", value };
  throw new DbError.Sql(`node:sqlite returned a value of an unknown kind (${typeof value})`);
}

/** One open database of {@link nodeSqliteDb}. */
class NodeSqliteConnection implements DbConnection {
  readonly #db: NodeDatabase;
  #closed = false;

  constructor(db: NodeDatabase) {
    this.#db = db;
  }

  /** Prepares exactly one statement and the arguments that bind `params` to it positionally. */
  #prepare(sql: string, params: readonly DbValue[]): { readonly statement: NodeStatement; readonly args: unknown[] } {
    if (this.#closed) throw new DbError.Unavailable("the database is closed");
    if (isTrivia(sql)) throw new DbError.Sql("the SQL holds no statement");
    let statement: NodeStatement;
    try {
      statement = this.#db.prepare(sql);
    } catch (error) {
      throw translate(error);
    }
    const source = statement.sourceSQL;
    const rest = sql.startsWith(source) ? sql.slice(source.length) : "";
    if (!isTrivia(rest)) throw new DbError.Sql("only one statement per call: use a migration for several");
    const { count, named } = scanParameters(source);
    if (params.length !== count) throw new DbError.Sql(`the statement has ${count} parameters, ${params.length} were given`);
    // node:sqlite binds anonymous values to the indices without a :name/@name/$name, in order, and
    // named ones through an object (SQLite's own numbering, so `?NNN` and repeats bind as SQLite says).
    const anonymous: NodeSqlValue[] = [];
    const byName: Record<string, NodeSqlValue> = {};
    for (let index = 1; index <= count; index++) {
      const value = toNode(params[index - 1] as DbValue);
      const name = named.get(index);
      if (name === undefined) anonymous.push(value);
      else byName[name] = value;
    }
    statement.setReadBigInts(true);
    return { statement, args: named.size > 0 ? [byName, ...anonymous] : anonymous };
  }

  async execute(sql: string, params: readonly DbValue[]): Promise<DbExecuted> {
    const { statement, args } = this.#prepare(sql, params);
    try {
      const done = statement.run(...args);
      return { changes: BigInt(done.changes), lastInsertId: BigInt(done.lastInsertRowid) };
    } catch (error) {
      throw translate(error);
    }
  }

  async query(sql: string, params: readonly DbValue[]): Promise<DbRows> {
    const { statement, args } = this.#prepare(sql, params);
    try {
      statement.setReturnArrays(true);
      const columns = statement.columns().map((column) => column.name);
      const rows = statement.all(...args).map((row) => row.map(fromNode));
      return { columns, rows };
    } catch (error) {
      throw translate(error);
    }
  }

  async executeScript(sql: string): Promise<void> {
    if (this.#closed) throw new DbError.Unavailable("the database is closed");
    try {
      this.#db.exec(sql);
    } catch (error) {
      throw translate(error);
    }
  }

  async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    try {
      this.#db.close();
    } catch {
      // Already closed.
    }
  }
}

/** Options of {@link nodeSqliteDb}. */
export interface NodeSqliteDbOptions {
  /** Where the database files go: `<directory>/<name>.sqlite`, the directory created when missing. */
  readonly directory: string;
}

/**
 * The Db adapter of Node (ADR-048): the built-in `node:sqlite` (`DatabaseSync`, Node 22.5+), one
 * connection per database, in `<directory>/<name>.sqlite` (`":memory:"` in memory). Integers cross
 * as `bigint` (`setReadBigInts`), every cell keeps its storage class, errors map by SQLite's result
 * code. `DatabaseSync` is synchronous: each statement runs on Node's thread, one at a time per
 * database (the binding's queue), so keep statements short or move heavy work to a worker.
 *
 * ```ts
 * import { UndraCore } from "@undra/runtime";
 * import { OptInPortIds, dbPort, nodeSqliteDb } from "@undra/runtime/db";
 *
 * await UndraCore.load({ ..., ports: { [OptInPortIds.Db.portId]: dbPort(nodeSqliteDb({ directory: "./data" })) } });
 * ```
 */
export function nodeSqliteDb(options: NodeSqliteDbOptions): DbAdapter {
  const directory = options.directory.replace(/[\\/]+$/, "");
  let made: Promise<unknown> | null = null;
  return {
    async open(name) {
      let sqlite: NodeSqliteModule;
      try {
        sqlite = nodeBuiltin<NodeSqliteModule>("node:sqlite");
      } catch (error) {
        throw new DbError.Unavailable(`node:sqlite is not available (Node 22.5 or later): ${errorMessage(error)}`);
      }
      let path = ":memory:";
      if (name !== ":memory:") {
        try {
          made ??= nodeBuiltin<NodeFsModule>("node:fs/promises").mkdir(directory, { recursive: true });
          await made;
        } catch (error) {
          made = null;
          throw new DbError.Unavailable(`cannot create ${directory}: ${errorMessage(error)}`);
        }
        path = `${directory}/${name}.sqlite`;
      }
      try {
        return new NodeSqliteConnection(new sqlite.DatabaseSync(path));
      } catch (error) {
        const failure = translate(error);
        throw failure instanceof DbError.Sql ? new DbError.Unavailable(failure.message_) : failure;
      }
    },
  };
}
