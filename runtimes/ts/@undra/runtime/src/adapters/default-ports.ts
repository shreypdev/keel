import type { PortImpl } from "../port.js";
import { QUIETLY_UNAVAILABLE } from "../port-dispatch.js";
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

/**
 * A port that builds its implementation on the first call and keeps it (a failed build is tried again by the next call). It
 * lists no method ids (they would have to be hashed from names at load, or written out): any id is forwarded to the port once
 * it is loaded, and one the real port does not have is answered "unavailable", quietly, as a port without that method is.
 */
function lazyPort(name: string, build: () => Promise<PortImpl>): PortImpl {
  let built: Promise<PortImpl> | undefined;
  const load = (): Promise<PortImpl> =>
    (built ??= build().catch((error: unknown) => {
      built = undefined;
      throw error;
    }));
  const methods = new Proxy({} as Record<number, Method>, {
    get: (_, id) =>
      typeof id === "string" && Number.isInteger(+id)
        ? async (args: Uint8Array) => {
            const method = (await load()).methods[+id];
            if (method === undefined) throw QUIETLY_UNAVAILABLE;
            return method(args);
          }
        : undefined,
  });
  return { name, sync: false, methods };
}

/**
 * The default ports for a core started with `given` (`LoadOptions.adapters`): the port of an adapter that
 * is `null` is left out, one that is given is served over it, the rest over the browser's own (Http only
 * where there is a global `fetch`). `namespace` is the core's: the browser's own Kv, SecureStore and Fs, built on the first
 * call, keep their data under it (SPEC 8, ADR-044 amendment A; the caller has checked it).
 */
export function defaultPorts(given: AdapterOverrides | undefined, namespace?: string): Map<number, PortImpl> {
  const ports = new Map<number, PortImpl>();
  for (const [key, name, portId] of DEFAULT_PORTS) {
    const adapter: Given = given?.[key];
    if (adapter === null || (adapter === undefined && key === "http" && typeof (globalThis as { fetch?: unknown }).fetch !== "function")) continue;
    ports.set(portId, lazyPort(name, () => import("./standard.js").then((m) => m.standardPort(key, adapter, namespace))));
  }
  return ports;
}
