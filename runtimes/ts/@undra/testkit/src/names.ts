import { fnv1a32 } from "@undra/runtime";

/** `(trait, methods)` of the ten standard ports (docs/SPEC.md section 8). */
const STANDARD: ReadonlyArray<readonly [string, readonly string[]]> = [
  ["Clock", ["now_ms", "monotonic_ns"]],
  ["Rng", ["fill"]],
  ["Log", ["log"]],
  ["Http", ["request"]],
  ["Kv", ["get", "set", "delete", "list"]],
  ["SecureStore", ["get", "set", "delete", "list"]],
  ["Fs", ["read", "write", "delete", "list"]],
  ["Timer", ["set"]],
  ["Connectivity", ["changed"]],
  ["Lifecycle", ["changed"]],
];

/** `"Http.request"` for the standard port method `(port, method)`, `undefined` for anything else. The ids stay authoritative; the name is for the person reading a recording. */
export function standardName(port: number, method: number): string | undefined {
  for (const [name, methods] of STANDARD) {
    if (fnv1a32(`port.${name}`) !== port) continue;
    for (const m of methods) if (fnv1a32(`${name}.${m}`) === method) return `${name}.${m}`;
  }
  return undefined;
}

/** The port id of `Trait`: `fnv1a32("port.<Trait>")`. */
export function portId(trait: string): number {
  return fnv1a32(`port.${trait}`);
}

/** The method id of `Trait.method`: `fnv1a32("<Trait>.<method>")`. */
export function methodId(trait: string, method: string): number {
  return fnv1a32(`${trait}.${method}`);
}
