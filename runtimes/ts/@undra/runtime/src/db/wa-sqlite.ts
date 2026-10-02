import type { DbAdapter } from "./binding.js";
import { type DbWorkerLike, workerDbAdapter } from "./protocol.js";

/** Options of {@link waSqliteDb}. */
export interface WaSqliteDbOptions {
  /**
   * The worker that serves the databases (`@undra/runtime/db-worker`, or a script that calls its
   * `startDbWorker` with options), or a function that makes it; made on the first `open`. Default:
   * a module worker on `@undra/runtime/db-worker`.
   */
  readonly worker?: DbWorkerLike | (() => DbWorkerLike);
}

/**
 * The browser's Db adapter (ADR-048 §7): SQLite (wa-sqlite, MIT, an optional peer dependency) in a
 * dedicated worker over the origin private file system (`AccessHandlePoolVFS`, `undra/<namespace>/db/`, the namespace being
 * that of the core the port is registered with: two cores of one page never share a database, ADR-044 amendment A), no
 * COOP/COEP needed. Every call crosses to the worker by `postMessage`; integers stay `bigint`, errors
 * stay typed by SQLite's result code. The web has no WAL: register it as
 * `dbPort(waSqliteDb(), { wal: false })`.
 *
 * ```ts
 * import { UndraCore } from "@undra/runtime";
 * import { OptInPortIds, dbPort, waSqliteDb } from "@undra/runtime/db";
 *
 * await UndraCore.load({ ..., ports: { [OptInPortIds.Db.portId]: dbPort(waSqliteDb(), { wal: false }) } });
 * ```
 *
 * The worker starts on the first `open`. Where it cannot (no `Worker`, its script failed, no OPFS
 * in it) every call is `DbError.Unavailable` saying why.
 */
export function waSqliteDb(options: WaSqliteDbOptions = {}): DbAdapter {
  return workerDbAdapter(() => {
    const given = options.worker;
    if (given !== undefined) return typeof given === "function" ? given() : given;
    if (typeof Worker !== "function") throw new Error("this platform has no Worker: pass `worker`");
    return new Worker(new URL("../db-worker.js", import.meta.url), { type: "module" }) as DbWorkerLike;
  });
}
