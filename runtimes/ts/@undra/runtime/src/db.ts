/*
 * `@undra/runtime/db`: the opt-in `Db` port of ADR-048, kept out of the main entry so a
 * hello-world bundle does not carry it (ADR-052). The records, `DbError` and their codecs
 * (`DbValue`, `DbRows`, ...) are in the main entry; this entry has the binding, the adapters and
 * `OptInPortIds`, the ids it is registered under.
 *
 * Register it before the first use, one line:
 *
 * ```ts
 * import { UndraCore } from "@undra/runtime";
 * import { OptInPortIds, dbPort, nodeSqliteDb } from "@undra/runtime/db";
 *
 * await UndraCore.load({
 *   mode: "wasm-main",
 *   wasm,
 *   expectedSchemaHash: UndraIds.schemaHash,
 *   ports: { [OptInPortIds.Db.portId]: dbPort(nodeSqliteDb({ directory: "./data" })) },
 * });
 * ```
 *
 * In the browser, `dbPort(waSqliteDb(), { wal: false })`: SQLite in a dedicated worker over OPFS
 * (`@undra/runtime/db-worker`, which needs the optional peer dependency wa-sqlite).
 *
 * In `wasm-worker` mode the port runs here, on the main thread, like every asynchronous port
 * (ADR-049 §2).
 */
export {
  dbPort,
  MAX_DB_NAME_LENGTH,
  validateDbName,
  validateMigrations,
  type DbAdapter,
  type DbConnection,
  type DbPortOptions,
} from "./db/binding.js";
export { OptInPortIds } from "./adapters/opt-in-ids.js";
export { nodeSqliteDb, type NodeSqliteDbOptions } from "./db/node-sqlite.js";
export { sqliteError } from "./db/errors.js";
export { waSqliteDb, type WaSqliteDbOptions } from "./db/wa-sqlite.js";
export {
  serveDb,
  workerDbAdapter,
  type DbWorkerLike,
  type DbWorkerReply,
  type DbWorkerRequest,
  type DbWorkerScope,
} from "./db/protocol.js";
