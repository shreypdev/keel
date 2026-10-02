import type { PortImpl } from "../port.js";
import { FS_PORT, HTTP_PORT, KV_PORT, SECURE_STORE_PORT } from "./port-literals.js";
import type { AdapterOverrides, FsAdapter, HttpAdapter, KvAdapter } from "./types.js";

/*
 * The four request/reply ports every core gets by default (Http, Kv, SecureStore, Fs), registered without
 * their code: a page whose core never calls one of them does not load its port, its codecs, its error
 * types or the browser adapter behind it (ADR-052). The first call to any of them loads `./standard.js`
 * and builds the port (once); the call then runs as it always did. All four are asynchronous ports, so a
 * call that waits for the module is an ordinary asynchronous port call.
 */

type Given = HttpAdapter | KvAdapter | FsAdapter | null | undefined;
type Method = (args: Uint8Array) => Promise<Uint8Array>;

/** The adapters of {@link AdapterOverrides} that back a default port, with the port each backs. */
const DEFAULT_PORTS = [
  ["http", "Http", HTTP_PORT],
  ["kv", "Kv", KV_PORT],
  ["secureStore", "SecureStore", SECURE_STORE_PORT],
  ["fs", "Fs", FS_PORT],
] as const;

/** A port that builds its implementation on the first call and keeps it (a failed build is tried again by the next call). */
function lazyPort(name: string, build: () => Promise<PortImpl>): PortImpl {
  let built: Promise<PortImpl> | undefined;
  const load = (): Promise<PortImpl> =>
    (built ??= build().catch((error: unknown) => {
      built = undefined;
      throw error;
    }));
  // Any method id: the real port answers once it is loaded (an id it does not have answers empty, as before).
  const methods = new Proxy({} as Record<number, Method>, {
    get: (_, id) => async (args: Uint8Array) => (await (await load()).methods[Number(id)]?.(args)) ?? new Uint8Array(0),
  });
  return { name, sync: false, methods };
}

export function defaultPorts(given: AdapterOverrides | undefined, namespace?: string): Map<number, PortImpl> {
  const ports = new Map<number, PortImpl>();
  for (const [key, name, portId] of DEFAULT_PORTS) {
    const adapter: Given = given?.[key];
    if (adapter === null || (adapter === undefined && key === "http" && typeof (globalThis as { fetch?: unknown }).fetch !== "function")) continue;
    ports.set(portId, lazyPort(name, () => import("./standard.js").then((m) => m.standardPort(key, adapter, namespace))));
  }
  return ports;
}
