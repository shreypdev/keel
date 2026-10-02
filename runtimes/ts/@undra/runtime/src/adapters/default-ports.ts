import type { PortImpl } from "../port.js";
import { PortIds } from "./ids.js";
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
  ["http", "Http"],
  ["kv", "Kv"],
  ["secureStore", "SecureStore"],
  ["fs", "Fs"],
] as const;

/** A port that builds its implementation on the first call and keeps it (a failed build is tried again by the next call). */
function lazyPort(name: string, ids: Readonly<Record<string, number>>, build: () => Promise<PortImpl>): PortImpl {
  let built: Promise<PortImpl> | undefined;
  const load = (): Promise<PortImpl> =>
    (built ??= build().catch((error: unknown) => {
      built = undefined;
      throw error;
    }));
  const methods: Record<number, Method> = {};
  for (const [key, id] of Object.entries(ids)) {
    if (key !== "portId") methods[id] = async (args) => (await (await load()).methods[id]?.(args)) ?? new Uint8Array(0);
  }
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
  for (const [key, name] of DEFAULT_PORTS) {
    const adapter: Given = given?.[key];
    if (adapter === null || (adapter === undefined && key === "http" && typeof (globalThis as { fetch?: unknown }).fetch !== "function")) continue;
    const ids = PortIds[name];
    ports.set(ids.portId, lazyPort(name, ids, () => import("./standard.js").then((m) => m.standardPort(key, adapter, namespace))));
  }
  return ports;
}
