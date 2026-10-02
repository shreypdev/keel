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
 * `UNDRA_BENCH_SCALE` of `env`: the factor a slower machine multiplies every latency ceiling by, `1` when it is unset.
 * It is read as `bench/tests/budgets.rs` reads it: a positive, finite decimal number, and anything else (an empty
 * string, `fast`, `0`, `-2`) is an error, so a typo cannot quietly leave a budget unscaled or switch it off.
 *
 * @param {Record<string, string | undefined>} [env]
 * @returns {number}
 */
export function benchScale(env = process.env) {
  const text = env.UNDRA_BENCH_SCALE;
  if (text === undefined) return 1;
  const scale = /^\+?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][+-]?[0-9]+)?$/.test(text) ? Number(text) : Number.NaN;
  if (!(Number.isFinite(scale) && scale > 0)) throw new Error(`UNDRA_BENCH_SCALE must be a positive number, not \`${text}\``);
  return scale;
}

/**
 * The web budgets of this checkout's bench/budgets.toml, each multiplied by `scale` (by default `UNDRA_BENCH_SCALE`: a
 * slower machine, the same rule the host budgets use, applied to ceilings and to nothing else).
 *
 * @param {number} [scale]
 * @returns {Record<string, { budgetNs: number, measuredNs: number | undefined }>}
 */
export function loadWebBudgets(scale = benchScale()) {
  const rows = parseWebBudgets(readFileSync(BUDGETS_PATH, "utf8"));
  for (const row of Object.values(rows)) row.budgetNs *= scale;
  return rows;
}
