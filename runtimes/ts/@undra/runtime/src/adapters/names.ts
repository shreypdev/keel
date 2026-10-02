/**
 * Where the default adapters of the web keep their data (SPEC 8, ADR-044 amendment A): per core namespace, so two
 * cores in one page (one origin) never share a store unless the app hands them one of its own.
 *
 * | Store | Default |
 * |---|---|
 * | `Kv` | IndexedDB database `undra.<namespace>.kv` |
 * | `SecureStore` | IndexedDB databases `undra.<namespace>.secure` and `undra.<namespace>.secure-keys` |
 * | `Fs` | the origin private file system directory `undra/<namespace>/fs` |
 * | `Db` (wa-sqlite) | the origin private file system directory `undra/<namespace>/db` |
 */

import { UndraError } from "../base-error.js";

/** The longest core namespace (`undra.toml` `[core] namespace`, SPEC 13): it is a path component and part of a C symbol. */
export const MAX_NAMESPACE_LENGTH = 32;

/**
 * Why `namespace` cannot name a core, or `undefined` when it can: the rule of `undra.toml`, a lowercase letter then
 * lowercase letters, digits and `_`, at most {@link MAX_NAMESPACE_LENGTH}. The namespace becomes a path component of the
 * default stores (`undra/<namespace>/fs`, `undra.<namespace>.kv`), so a `..`, a `/`, a NUL, an empty or a very long one
 * is refused before it reaches IndexedDB, OPFS or the database worker.
 */
export function namespaceProblem(namespace: string): string | undefined {
  if (namespace.length === 0) return "it is empty";
  if (namespace.length > MAX_NAMESPACE_LENGTH) return `it is ${namespace.length} characters long, and a namespace has at most ${MAX_NAMESPACE_LENGTH}`;
  if (!/^[a-z]/.test(namespace)) return "it must start with a lowercase letter";
  const bad = /[^a-z0-9_]/u.exec(namespace);
  if (bad !== null) return `${JSON.stringify(bad[0])} is not allowed (lowercase letters, digits and \`_\` only)`;
  return undefined;
}

/**
 * Checks the namespace an app gave `load`, `attach` or an adapter, if it gave one.
 *
 * @throws UndraError `kind` `"options"`, saying what is wrong and where the namespace comes from.
 */
export function checkNamespace(namespace: string | undefined): void {
  if (namespace === undefined) return;
  const problem = typeof namespace === "string" ? namespaceProblem(namespace) : "it is not a string";
  if (problem !== undefined) {
    const shown = typeof namespace === "string" ? JSON.stringify(namespace.length > 40 ? `${namespace.slice(0, 40)}…` : namespace) : typeof namespace;
    throw new UndraError(
      "options",
      `\`namespace\` ${shown} cannot name a core: ${problem}. It is the core's namespace, \`UndraIds.namespace\` (\`[core] namespace\` in undra.toml), which the generated \`Undra<Namespace>.load\` passes: lowercase letters, digits and \`_\`, starting with a letter, at most ${MAX_NAMESPACE_LENGTH}`,
    );
  }
}

/**
 * The namespace of a core that was loaded without one (`LoadOptions.namespace` unset, a transport the app provides
 * without its generated entry): `_`, which no real namespace is (a namespace starts with a lowercase letter), so it
 * never collides with a core's. Two cores loaded without a namespace share the stores of `_`; a generated entry always
 * sets one.
 */
export const UNNAMED_NAMESPACE = "_";

/** The name of the IndexedDB database of `store` for the core `namespace`: `undra.<namespace>.<store>`. */
export function storeName(namespace: string | undefined, store: string): string {
  return `undra.${storeNamespace(namespace)}.${store}`;
}

/** The origin-private-file-system directory of `store` for the core `namespace`: `undra/<namespace>/<store>`. */
export function storePath(namespace: string | undefined, store: string): string {
  return `undra/${storeNamespace(namespace)}/${store}`;
}

/** The namespace a store name is made from: a valid one, or {@link UNNAMED_NAMESPACE} for none. Never a `..` or a `/`. */
function storeNamespace(namespace: string | undefined): string {
  if (namespace === undefined || namespace === UNNAMED_NAMESPACE) return UNNAMED_NAMESPACE;
  checkNamespace(namespace);
  return namespace;
}
