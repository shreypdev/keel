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
import { msg } from "../messages.js";

/**
 * The namespace of a core that was loaded without one (`LoadOptions.namespace` unset, a transport the app provides
 * without its generated entry): `_`, which no real namespace is (a namespace starts with a lowercase letter), so it
 * never collides with a core's. Two cores loaded without a namespace share the stores of `_`; a generated entry always
 * sets one.
 */
export const UNNAMED_NAMESPACE = "_";

/**
 * The rule of `undra.toml` for a core namespace (SPEC 13): a lowercase letter, then lowercase letters, digits and `_`, at
 * most 32 (and `_`, the unnamed one). The namespace becomes a path component of the default stores
 * (`undra/<namespace>/fs`, `undra.<namespace>.kv`), so a `..`, a `/`, a NUL, an empty or a long one never gets that far.
 */
const NAMESPACE = /^(?:[a-z][a-z0-9_]{0,31}|_)$/;

/**
 * The namespace the names of a core's stores are made from: `namespace`, or {@link UNNAMED_NAMESPACE} when it is unset.
 *
 * @throws UndraError `kind` `"options"` when it is not a core namespace.
 */
export function checkNamespace(namespace: string | undefined): string {
  if (namespace === undefined) return UNNAMED_NAMESPACE;
  if (typeof namespace !== "string" || !NAMESPACE.test(namespace)) {
    throw new UndraError("options", msg(18, JSON.stringify(String(namespace).slice(0, 40))));
  }
  return namespace;
}

/** The name of the IndexedDB database of `store` for the core `namespace`: `undra.<namespace>.<store>`. */
export function storeName(namespace: string | undefined, store: string): string {
  return `undra.${checkNamespace(namespace)}.${store}`;
}

/** The origin-private-file-system directory of `store` for the core `namespace`: `undra/<namespace>/<store>`. */
export function storePath(namespace: string | undefined, store: string): string {
  return `undra/${checkNamespace(namespace)}/${store}`;
}
