/*
 * `@undra/runtime/db`: the opt-in `Db` port of ADR-048, kept out of the main entry so a
 * hello-world bundle does not carry it (ADR-052). The records, `DbError` and their codecs
 * (`DbValue`, `DbRows`, ...) and `PortIds.Db` are in the main entry; this entry has the binding and
 * the adapters.
 *
 * Register it before the first use, one line:
 *
 * ```ts
 * import { PortIds, UndraCore } from "@undra/runtime";
 * import { dbPort, nodeSqliteDb } from "@undra/runtime/db";
 *
 * await UndraCore.load({
 *   mode: "wasm-main",
 *   wasm,
 *   expectedSchemaHash: UndraIds.schemaHash,
 *   ports: { [PortIds.Db.portId]: dbPort(nodeSqliteDb({ directory: "./data" })) },
 * });
 * ```
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
export { nodeSqliteDb, sqliteError, type NodeSqliteDbOptions } from "./db/node-sqlite.js";
export { waSqliteDb, WA_SQLITE_PENDING, type WaSqliteDbOptions } from "./db/wa-sqlite.js";
