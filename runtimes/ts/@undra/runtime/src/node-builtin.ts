import { msg } from "./messages.js";
/*
 * Node's built-in modules for the Node-only adapters (`nodeWebSocket`, `nodeSqliteDb`), reached
 * through `process.getBuiltinModule` (Node 20.16+, 22.3+) instead of an `import()`: no bundler
 * (Vite, webpack, Metro) sees a module request, so the files that hold these adapters bundle for a
 * browser or React Native unchanged, and nothing is loaded until an adapter is used under Node.
 * Internal to `@undra/runtime/realtime` and `@undra/runtime/db`.
 */

/** Node's `process`, as far as this file reads it. */
interface NodeProcess {
  readonly versions?: { readonly node?: string };
  getBuiltinModule?(id: string): unknown;
}

/**
 * The built-in module `id` (`"node:http"`, ...). Throws an `Error` that says why when this is not
 * Node, the Node is older than 20.16 / 22.3, or it has no such module.
 */
export function nodeBuiltin<T>(id: string): T {
  const process = (globalThis as { process?: NodeProcess }).process;
  if (typeof process?.getBuiltinModule !== "function") {
    const node = process?.versions?.node;
    throw new Error(
      node === undefined ? msg(109, id) : msg(110, id, node),
    );
  }
  const module = process.getBuiltinModule(id);
  if (module === undefined || module === null) throw new Error(msg(111, id));
  return module as T;
}
