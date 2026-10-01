/** What changed between two values, in the smallest form worth showing. */

import { formatValue } from "./format.js";
import type { PatchOp, Value } from "./value.js";

export type Diff =
  | { readonly kind: "same" }
  | { readonly kind: "set"; readonly before: Value | undefined; readonly after: Value | undefined }
  | { readonly kind: "fields"; readonly changes: readonly { readonly key: string; readonly diff: Diff }[] }
  | {
      readonly kind: "list";
      readonly before: number;
      readonly after: number;
      /** The changed positions when the lists are short enough to compare item by item. */
      readonly items: readonly { readonly index: number; readonly diff: Diff }[] | undefined;
    };

/** Lists longer than this are compared by length only. */
const ITEMWISE_LIMIT = 200;
const MAX_ITEMS_SHOWN = 12;
const MAX_DEPTH = 4;

function isObject(v: Value | undefined): v is { [field: string]: Value } {
  return typeof v === "object" && v !== null && !Array.isArray(v) && !(v instanceof Uint8Array);
}

/** Structural equality of two values. */
export function equal(a: Value | undefined, b: Value | undefined): boolean {
  if (a === b) return true;
  if (a === undefined || b === undefined || a === null || b === null) return false;
  if (typeof a !== typeof b) return false;
  if (a instanceof Uint8Array || b instanceof Uint8Array) {
    return a instanceof Uint8Array && b instanceof Uint8Array && a.length === b.length && a.every((x, i) => x === b[i]);
  }
  if (Array.isArray(a) || Array.isArray(b)) {
    return Array.isArray(a) && Array.isArray(b) && a.length === b.length && a.every((x, i) => equal(x, b[i]));
  }
  if (isObject(a) && isObject(b)) {
    const ka = Object.keys(a);
    return ka.length === Object.keys(b).length && ka.every((k) => k in b && equal(a[k], b[k]));
  }
  return false;
}

/** Compares `before` with `after`. */
export function diffValues(before: Value | undefined, after: Value | undefined, depth = 0): Diff {
  if (equal(before, after)) return { kind: "same" };
  if (depth < MAX_DEPTH && Array.isArray(before) && Array.isArray(after)) {
    if (before.length > ITEMWISE_LIMIT || after.length > ITEMWISE_LIMIT) {
      return { kind: "list", before: before.length, after: after.length, items: undefined };
    }
    const items: { index: number; diff: Diff }[] = [];
    for (let i = 0; i < Math.max(before.length, after.length); i++) {
      const d = diffValues(before[i], after[i], depth + 1);
      if (d.kind !== "same") items.push({ index: i, diff: d });
    }
    return { kind: "list", before: before.length, after: after.length, items };
  }
  if (depth < MAX_DEPTH && isObject(before) && isObject(after) && before["$"] === after["$"]) {
    const keys = [...new Set([...Object.keys(before), ...Object.keys(after)])].filter((k) => k !== "$");
    const changes = keys
      .map((key) => ({ key, diff: diffValues(before[key], after[key], depth + 1) }))
      .filter((c) => c.diff.kind !== "same");
    return { kind: "fields", changes };
  }
  return { kind: "set", before, after };
}

/** A diff as lines of text, indented by nesting. */
export function diffLines(diff: Diff, indent = 0): string[] {
  const pad = "  ".repeat(indent);
  switch (diff.kind) {
    case "same":
      return [];
    case "set":
      return [`${pad}${formatValue(diff.before, 48)}  →  ${formatValue(diff.after, 48)}`];
    case "fields":
      return diff.changes.flatMap((c) => {
        const inner = diffLines(c.diff, indent + 1);
        return inner.length === 1 && c.diff.kind === "set"
          ? [`${pad}${c.key}: ${(inner[0] as string).trimStart()}`]
          : [`${pad}${c.key}:`, ...inner];
      });
    case "list": {
      const head = `${pad}${diff.before} → ${diff.after} items`;
      if (diff.items === undefined) return [head];
      const shown = diff.items.slice(0, MAX_ITEMS_SHOWN).flatMap((c) => {
        const inner = diffLines(c.diff, indent + 1);
        return inner.length === 1 && c.diff.kind === "set"
          ? [`${pad}[${c.index}] ${(inner[0] as string).trimStart()}`]
          : [`${pad}[${c.index}]`, ...inner];
      });
      const more = diff.items.length - MAX_ITEMS_SHOWN;
      return [head, ...shown, ...(more > 0 ? [`${pad}… ${more} more`] : [])];
    }
  }
}

/** The operations of a keyed patch as lines of text. */
export function patchLines(ops: readonly PatchOp[], max = 16): string[] {
  const lines = ops.slice(0, max).map((o) => {
    switch (o.op) {
      case "insert":
        return `+ [${o.index}] ${formatValue(o.item, 64)}`;
      case "remove":
        return `− [${o.index}]`;
      case "update":
        return `~ [${o.index}] ${formatValue(o.item, 64)}`;
      case "move":
        return `↷ [${o.from}] → [${o.to}]`;
      case "clear":
        return "clear";
    }
  });
  if (ops.length > max) lines.push(`… ${ops.length - max} more operations`);
  return lines;
}

/** `+2 −1 ~3` for a patch: how many of each. */
export function patchSummary(ops: readonly PatchOp[]): string {
  const n = { insert: 0, remove: 0, update: 0, move: 0, clear: 0 };
  for (const o of ops) n[o.op]++;
  const parts: string[] = [];
  if (n.clear > 0) parts.push("clear");
  if (n.insert > 0) parts.push(`+${n.insert}`);
  if (n.remove > 0) parts.push(`−${n.remove}`);
  if (n.update > 0) parts.push(`~${n.update}`);
  if (n.move > 0) parts.push(`↷${n.move}`);
  return parts.join(" ");
}
