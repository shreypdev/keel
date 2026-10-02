import SQLiteESMFactory from "wa-sqlite/dist/wa-sqlite.mjs";
import { AccessHandlePoolVFS } from "wa-sqlite/src/examples/AccessHandlePoolVFS.js";
import { MemoryVFS } from "wa-sqlite/src/examples/MemoryVFS.js";
import { Factory } from "wa-sqlite/src/sqlite-api.js";
import { DbError } from "./adapters/types.js";
import type { DbAdapter } from "./db/binding.js";
import { type DbWorkerScope, serveDb } from "./db/protocol.js";
import { waSqliteEngine } from "./db/wa-sqlite-engine.js";
import { nodeBuiltin } from "./node-builtin.js";
import { errorMessage } from "./platform.js";

/*
 * `@undra/runtime/db-worker`: SQLite for the browser's `Db` port (ADR-048 §7), wa-sqlite's
 * synchronous build over its OPFS `AccessHandlePoolVFS`, in a dedicated worker (OPFS access handles
 * exist only there; no COOP/COEP needed). `waSqliteDb()` (`@undra/runtime/db`) starts a module
 * worker on this file and proxies every call to it. wa-sqlite is an optional peer dependency: only
 * this entry imports it, never the main entry.
 *
 * Loaded as a dedicated worker's script, it serves with the defaults unless the script that
 * imported it called `startDbWorker` itself during its first run (to pass options):
 *
 * ```ts
 * // my-db-worker.ts
 * import { startDbWorker } from "@undra/runtime/db-worker";
 * startDbWorker(self, { directory: "my-app/db" });
 * // the page: waSqliteDb({ worker: () => new Worker(new URL("./my-db-worker.ts", import.meta.url), { type: "module" }) })
 * ```
 */

export { serveDb, type DbWorkerScope } from "./db/protocol.js";

/** Options of {@link waSqliteAdapter} and {@link startDbWorker}. */
export interface WaSqliteAdapterOptions {
  /**
   * Where the databases live: `"opfs"` (the origin private file system, persistent; dedicated
   * workers only; the default) or `"memory"` (this thread's memory, gone with it: tests, caches,
   * Node).
   */
  readonly storage?: "opfs" | "memory";
  /** The OPFS directory of the databases (each one is `<directory>/…` in the pool's files). Default `undra/db`. */
  readonly directory?: string;
  /**
   * The `wa-sqlite.wasm` to load: its bytes, or its URL. Default: the file next to wa-sqlite's glue
   * (bundlers emit it with the worker), read from disk under Node.
   */
  readonly wasm?: Uint8Array | URL | string;
}

/** Whether this is Node. */
function isNode(): boolean {
  return typeof (globalThis as { process?: { versions?: { node?: unknown } } }).process?.versions?.node === "string";
}

/** wa-sqlite.wasm from the installed package, under Node. */
function nodeWasm(): Uint8Array {
  const { createRequire } = nodeBuiltin<{ createRequire(from: string): { resolve(id: string): string } }>("node:module");
  const { readFileSync } = nodeBuiltin<{ readFileSync(path: string): Uint8Array }>("node:fs");
  return readFileSync(createRequire(import.meta.url).resolve("wa-sqlite/dist/wa-sqlite.wasm"));
}

function moduleConfig(wasm: WaSqliteAdapterOptions["wasm"]): { wasmBinary?: Uint8Array; locateFile?: (path: string) => string } {
  if (wasm instanceof Uint8Array) return { wasmBinary: wasm };
  if (wasm !== undefined) return { locateFile: () => String(wasm) };
  return isNode() ? { wasmBinary: nodeWasm() } : {};
}

/** Files a database may hold open at once in the OPFS pool (the database, its journal, a temporary file). */
const FILES_PER_DATABASE = 3;

/**
 * The Db adapter of wa-sqlite in this thread: OPFS (`storage: "opfs"`, a dedicated worker) or
 * memory. The module and the VFS load on the first `open`; when they cannot (no OPFS here, the
 * wasm did not load) every `open` is `DbError.Unavailable` saying why. Errors map by SQLite's
 * result code, integers cross as `bigint`.
 */
export function waSqliteAdapter(options: WaSqliteAdapterOptions = {}): DbAdapter {
  const storage = options.storage ?? "opfs";
  const directory = options.directory ?? "undra/db";
  let engine: Promise<DbAdapter> | null = null;
  const load = async (): Promise<DbAdapter> => {
    const module = await SQLiteESMFactory(moduleConfig(options.wasm));
    const api = Factory(module);
    const path = (name: string): string => `/${name}`;
    if (storage === "memory") {
      const vfs = new MemoryVFS();
      api.vfs_register(vfs, false);
      return waSqliteEngine(api, module, vfs.name, path);
    }
    const vfs = new AccessHandlePoolVFS(directory);
    await vfs.isReady;
    api.vfs_register(vfs, false);
    return waSqliteEngine(api, module, vfs.name, path, async () => {
      if (vfs.getCapacity() - vfs.getSize() < FILES_PER_DATABASE) await vfs.addCapacity(FILES_PER_DATABASE);
    });
  };
  return {
    async open(name) {
      engine ??= load();
      let ready: DbAdapter;
      try {
        ready = await engine;
      } catch (error) {
        throw new DbError.Unavailable(`wa-sqlite could not start (${storage}): ${errorMessage(error)}`);
      }
      return ready.open(name);
    },
  };
}

let started = false;

/**
 * Serves wa-sqlite databases on `scope` (default this worker's global scope) for `waSqliteDb()`.
 * Returns the function that stops serving. Called at most once per worker.
 */
export function startDbWorker(scope?: DbWorkerScope, options: WaSqliteAdapterOptions = {}): () => void {
  started = true;
  return serveDb(scope ?? (globalThis as unknown as DbWorkerScope), waSqliteAdapter(options));
}

/** Whether this is a dedicated worker's global scope. */
function isDedicatedWorker(scope: unknown): boolean {
  const ctor = (scope as { DedicatedWorkerGlobalScope?: new () => unknown }).DedicatedWorkerGlobalScope;
  return typeof ctor === "function" && scope instanceof ctor;
}

// The worker script `waSqliteDb()` starts: serve with the defaults, unless the script that imported
// this module started it with options while it ran (a task boundary has not passed yet).
if (isDedicatedWorker(globalThis)) {
  queueMicrotask(() => {
    if (!started) startDbWorker();
  });
}
