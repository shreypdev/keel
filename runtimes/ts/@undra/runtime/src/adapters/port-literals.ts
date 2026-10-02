// PROTOTYPE (ADR-057 lever d3). The few standard-port ids the first chunk needs, as literals: `PortIds` (ids.ts) computes
// every id from its name and stays the public table; `port-literals.test.ts` pins each of these to it.
/** `PortIds.Http.portId`, `Kv`, `SecureStore`, `Fs`: the ports that load on their first call. */
export const HTTP_PORT = 0x1ebeb908;
export const KV_PORT = 0x5389110d;
export const SECURE_STORE_PORT = 0xc01f5bea;
export const FS_PORT = 0x4ea34cab;
/** `PortIds.Connectivity.portId` and `.changed`; `PortIds.Lifecycle.portId` and `.changed`. */
export const CONNECTIVITY_PORT = 0x1feff6ff;
export const CONNECTIVITY_CHANGED = 0xb4f2a010;
export const LIFECYCLE_PORT = 0x81c0afd4;
export const LIFECYCLE_CHANGED = 0x0bc82569;
/** `PortIds.Timer.portId`. */
export const TIMER_PORT = 0x00c2cdd9;
