// The `[web."id"]` tables of bench/budgets.toml (R9: the web call path's budgets are tests), for the two
// harnesses that measure them in JavaScript: the playground's device bench in Chromium
// (examples/playground/web/bench/bench.spec.ts) and the runtime's call-path guard
// (runtimes/ts/@undra/runtime/test/call-path.test.ts). bench/src/budget.rs reads the same tables
// (budget_ns, measured_ns, what) and checks them against themselves; this reads the first two.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

/** The path of bench/budgets.toml of this checkout. */
export const BUDGETS_PATH = fileURLToPath(new URL("../bench/budgets.toml", import.meta.url));

/**
 * The web budgets of `text` (the budgets file): `{ id: { budgetNs, measuredNs } }`. The file is a strict subset of TOML
 * (`key = number` lines, `#` comments, `_` separators), so a line each is all that is parsed.
 *
 * @param {string} text
 * @returns {Record<string, { budgetNs: number, measuredNs: number | undefined }>}
 */
export function parseWebBudgets(text) {
  /** @type {Record<string, { budgetNs: number, measuredNs: number | undefined }>} */
  const rows = {};
  /** @type {string | undefined} */
  let id;
  for (const raw of text.split("\n")) {
    const line = raw.replace(/#.*$/, "").trim();
    const header = /^\[web\."([^"]+)"\]$/.exec(line);
    if (header) {
      id = header[1];
      continue;
    }
    if (line.startsWith("[")) {
      id = undefined;
      continue;
    }
    if (id === undefined) continue;
    const number = /^(budget_ns|measured_ns)\s*=\s*([0-9_.]+)$/.exec(line);
    if (!number) continue;
    const row = (rows[id] ??= { budgetNs: Number.NaN, measuredNs: undefined });
    const value = Number(number[2].replaceAll("_", ""));
    if (number[1] === "budget_ns") row.budgetNs = value;
    else row.measuredNs = value;
  }
  for (const [name, row] of Object.entries(rows)) {
    if (!(row.budgetNs > 0)) throw new Error(`bench/budgets.toml: [web."${name}"] has no budget_ns`);
  }
  return rows;
}

/**
 * The web budgets of this checkout's bench/budgets.toml, each multiplied by `UNDRA_BENCH_SCALE` (a slower machine, the
 * same rule the host budgets use) when it is set.
 *
 * @returns {Record<string, { budgetNs: number, measuredNs: number | undefined }>}
 */
export function loadWebBudgets() {
  const scale = Number(process.env.UNDRA_BENCH_SCALE ?? 1);
  const rows = parseWebBudgets(readFileSync(BUDGETS_PATH, "utf8"));
  const factor = Number.isFinite(scale) && scale > 0 ? scale : 1;
  for (const row of Object.values(rows)) row.budgetNs *= factor;
  return rows;
}
