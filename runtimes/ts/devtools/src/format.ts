/** Showing values: one line for a row of a table, indented JSON for a detail view. */

import type { Value } from "./value.js";

function isObject(v: Value): v is { [field: string]: Value } {
  return typeof v === "object" && v !== null && !Array.isArray(v) && !(v instanceof Uint8Array);
}

function hexOf(bytes: Uint8Array, max: number): string {
  let out = "";
  for (let i = 0; i < Math.min(bytes.length, max); i++) out += (bytes[i] as number).toString(16).padStart(2, "0");
  return bytes.length > max ? `${out}…` : out;
}

/** The ISO form of a millisecond timestamp, or the number when it is not a date. */
export function isoOf(ms: number): string {
  const d = new Date(ms);
  return Number.isNaN(d.getTime()) ? String(ms) : d.toISOString();
}

/** A duration in nanoseconds as `1.5 ms`. */
export function durationText(ns: number | bigint): string {
  const n = Number(ns);
  if (n >= 1e9) return `${(n / 1e9).toFixed(2)} s`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)} ms`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)} us`;
  return `${n} ns`;
}

/** A value on one line, cut at `max` characters. */
export function formatValue(v: Value | undefined, max = 80): string {
  const text = inline(v, max);
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

function inline(v: Value | undefined, budget: number): string {
  if (v === undefined) return "·";
  if (v === null) return "null";
  switch (typeof v) {
    case "string":
      return JSON.stringify(v);
    case "bigint":
      return `${v}n`;
    case "number":
    case "boolean":
      return String(v);
    default:
      break;
  }
  if (v instanceof Uint8Array) return `bytes(${v.length}) ${hexOf(v, 8)}`;
  if (Array.isArray(v)) {
    if (v.length === 0) return "[]";
    const parts: string[] = [];
    let used = 2;
    for (const item of v) {
      const s = inline(item, budget - used);
      parts.push(s);
      used += s.length + 2;
      if (used > budget) return `[${parts.join(", ")}, … ${v.length} items]`;
    }
    return `[${parts.join(", ")}]`;
  }
  if (isObject(v)) {
    if ("$ts" in v) return isoOf(Number(v["$ts"]));
    if ("$dur" in v) return durationText(v["$dur"] as number | bigint);
    if ("$handle" in v) return `handle ${String(v["$handle"])}`;
    if ("$lazy" in v) return `lazy ${String(v["$lazy"])}`;
    if ("$map" in v) {
      const pairs = v["$map"] as Value[];
      return `{${pairs.map((p) => (Array.isArray(p) ? `${inline(p[0], 30)}: ${inline(p[1], 40)}` : "?")).join(", ")}}`.slice(0, budget + 1);
    }
    const tagged = typeof v["$"] === "string" ? (v["$"] as string) : undefined;
    const fields = Object.entries(v).filter(([k]) => k !== "$");
    const body = fields.map(([k, val]) => (tagged !== undefined && /^\d+$/.test(k) ? inline(val, 40) : `${k}: ${inline(val, 40)}`)).join(", ");
    if (tagged !== undefined) return fields.length === 0 ? tagged : `${tagged}(${body})`;
    return `{${body}}`;
  }
  return String(v);
}

/** A value as indented, readable text (bytes in hex, bigints with an `n`). */
export function pretty(v: Value | undefined, indent = 0): string {
  const pad = "  ".repeat(indent);
  if (v === undefined) return "·";
  if (v === null || typeof v !== "object" || v instanceof Uint8Array) return inline(v, 400);
  if (Array.isArray(v)) {
    if (v.length === 0) return "[]";
    if (v.every((x) => x === null || typeof x !== "object") && v.length <= 8) return inline(v, 400);
    return `[\n${v.map((x) => `${pad}  ${pretty(x, indent + 1)}`).join(",\n")}\n${pad}]`;
  }
  if ("$ts" in v || "$dur" in v || "$handle" in v || "$lazy" in v) return inline(v, 400);
  const entries = Object.entries(v);
  if (entries.length === 0) return "{}";
  return `{\n${entries.map(([k, x]) => `${pad}  ${k}: ${pretty(x, indent + 1)}`).join(",\n")}\n${pad}}`;
}
