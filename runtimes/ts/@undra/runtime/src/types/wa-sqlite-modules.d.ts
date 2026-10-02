/*
 * The parts of wa-sqlite (MIT, an optional peer dependency) that `@undra/runtime/db-worker` imports,
 * declared here: wa-sqlite's own declarations are global and loose, and the runtime's exported
 * types never mention wa-sqlite, so an app without it type-checks.
 */

declare module "wa-sqlite/dist/wa-sqlite.mjs" {
  /** The Emscripten module factory of wa-sqlite's synchronous build. */
  const factory: (config?: {
    readonly wasmBinary?: Uint8Array;
    readonly locateFile?: (path: string) => string;
  }) => Promise<import("../db/wa-sqlite-engine.js").SqliteModule>;
  export default factory;
}

declare module "wa-sqlite/src/sqlite-api.js" {
  /** Builds the JavaScript API over the Emscripten module. */
  export function Factory(module: import("../db/wa-sqlite-engine.js").SqliteModule): import("../db/wa-sqlite-engine.js").SqliteApi & {
    vfs_register(vfs: object, makeDefault?: boolean): number;
  };
}

declare module "wa-sqlite/src/examples/MemoryVFS.js" {
  /** wa-sqlite's in-memory VFS. */
  export class MemoryVFS {
    readonly name: string;
    close(): void;
  }
}

declare module "wa-sqlite/src/examples/AccessHandlePoolVFS.js" {
  /** wa-sqlite's OPFS VFS over a pool of synchronous access handles (dedicated workers only). */
  export class AccessHandlePoolVFS {
    constructor(directoryPath: string);
    readonly name: string;
    readonly isReady: Promise<void>;
    getSize(): number;
    getCapacity(): number;
    addCapacity(n: number): Promise<number>;
    close(): Promise<void>;
  }
}
