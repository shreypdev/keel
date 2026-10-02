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

/**
 * The namespace of a core that was loaded without one (`LoadOptions.namespace` unset, a transport the app provides
 * without its generated entry): `_`, which no real namespace is (a namespace starts with a lowercase letter), so it
 * never collides with a core's.
 */
export const UNNAMED_NAMESPACE = "_";

/** The name of the IndexedDB database of `store` for the core `namespace`: `undra.<namespace>.<store>`. */
export function storeName(namespace: string | undefined, store: string): string {
  return `undra.${namespace ?? UNNAMED_NAMESPACE}.${store}`;
}

/** The origin-private-file-system directory of `store` for the core `namespace`: `undra/<namespace>/<store>`. */
export function storePath(namespace: string | undefined, store: string): string {
  return `undra/${namespace ?? UNNAMED_NAMESPACE}/${store}`;
}
