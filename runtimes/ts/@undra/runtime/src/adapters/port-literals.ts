/*
 * The standard-port ids the first chunk needs, as literals (ADR-057, lever 2). `PortIds` (`ids.ts`) computes every id from its
 * name with `fnv1a32` and stays the public table; a hello page needs nine of those numbers and would otherwise ship the hash
 * function, the table of every standard port and the name of every method to compute them at load (and one standard function id). Nothing in the first chunk
 * imports `ids.ts` or `fnv.ts`. `test/port-literals.test.ts` derives each of these from the name in `PortIds`, so a literal
 * cannot drift from the schema's rule (`port_id = fnv1a32("port.<Trait>")`, SPEC 1.1).
 */

/** `PortIds.Http.portId`, `Kv`, `SecureStore`, `Fs`: the ports that load on their first call. */
export const HTTP_PORT = 0x1ebeb908;
export const KV_PORT = 0x5389110d;
export const SECURE_STORE_PORT = 0xc01f5bea;
export const FS_PORT = 0x4ea34cab;
/** `PortIds.Connectivity.portId` and `.changed`; `PortIds.Lifecycle.portId` and `.changed`: the host events. */
export const CONNECTIVITY_PORT = 0x1feff6ff;
export const CONNECTIVITY_CHANGED = 0xb4f2a010;
export const LIFECYCLE_PORT = 0x81c0afd4;
export const LIFECYCLE_CHANGED = 0x0bc82569;
/** `PortIds.Timer.portId`: the Timer port an explicit adapter of a native core serves. */
export const TIMER_PORT = 0x00c2cdd9;
/**
 * The standard function `run_background` (`fnv1a32("fn.run_background")`, ADR-046): in every schema, called like any free async
 * function. The page's background window calls it from the first chunk, so a page that is being left needs no chunk to drain.
 */
export const RUN_BACKGROUND = 0x0e5b14ff;
