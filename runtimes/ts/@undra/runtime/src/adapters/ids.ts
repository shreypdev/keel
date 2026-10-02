import { fnv1a32 } from "../fnv.js";

/*
 * Stable identifiers of the standard ports (docs/SPEC.md sections 1.1 and 8):
 * `port_id = fnv1a32("port.<Trait>")`, `method_id = fnv1a32("<Trait>.<method>")`.
 * They are computed here, not copied, so a typo cannot drift from the macros.
 */

/** `nowMs` to `now_ms`: method ids hash the Rust name of the method. */
function snakeCase(name: string): string {
  return name.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);
}

function port<const M extends readonly string[]>(name: string, methods: M) {
  const ids = { portId: fnv1a32(`port.${name}`) } as { portId: number } & { readonly [K in M[number]]: number };
  for (const m of methods) (ids as Record<string, number>)[m] = fnv1a32(`${name}.${snakeCase(m)}`);
  return Object.freeze(ids);
}

/** Port and method ids of the eleven standard ports. */
export const PortIds = Object.freeze({
  Clock: port("Clock", ["nowMs", "monotonicNs"] as const),
  Rng: port("Rng", ["fill"] as const),
  Log: port("Log", ["log"] as const),
  Http: port("Http", ["request"] as const),
  Kv: port("Kv", ["get", "set", "delete", "list"] as const),
  SecureStore: port("SecureStore", ["get", "set", "delete", "list"] as const),
  Fs: port("Fs", ["read", "write", "delete", "list"] as const),
  Timer: port("Timer", ["set"] as const),
  Connectivity: port("Connectivity", ["changed"] as const),
  Lifecycle: port("Lifecycle", ["changed"] as const),
  Diagnostics: port("Diagnostics", ["panicked"] as const),
});
