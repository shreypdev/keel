#!/usr/bin/env node
// Renders the benchmark table of the landing page from site/data/bench.json, at build time.
// Writes between <!-- numbers:start --> and <!-- numbers:end --> in site/index.html, and shows the
// "Push it" button of the live demo only when bench.json says the stress screen exists.
//
//   node site/scripts/build-numbers.mjs
//
// bench.json: { machine, method, source, stressScreen, rows: [Row], harsh: [Row] } where a Row is
// { id, operation, value, unit, budget, budgetUnit, gate|null, source }. Units: ns, µs, ms, KB, MB.
import { join } from "node:path";
import { SITE, read, writeIfChanged, replaceRegion, esc } from "./lib.mjs";

const FACTOR = { ns: 1, "µs": 1e3, ms: 1e6, KB: 1, MB: 1e3 };
const fmtNum = (n) => n.toLocaleString("en-US", { maximumFractionDigits: 2 });
const pct = (r) => (r >= 0.1 ? Math.round(r * 100) + "%" : (r * 100).toFixed(1) + "%");

function ratio(row) {
  const a = FACTOR[row.unit], b = FACTOR[row.budgetUnit];
  if (!a || !b || !(row.budget > 0)) throw new Error(`bench row ${row.id}: unknown unit or budget`);
  return (row.value * a) / (row.budget * b);
}

function tableRow(row) {
  const r = ratio(row);
  const label = new URL(row.source).pathname.split("/").pop();
  const gate = row.gate ? `<code>${esc(row.gate)}</code>` : `<span class="tok-dim">reported by <code>keel build</code></span>`;
  return [
    "<tr>",
    `<th scope="row">${esc(row.operation)}</th>`,
    `<td class="r" data-l="Measured"><span class="val">${esc(fmtNum(row.value))} ${esc(row.unit)}</span><span class="use" role="img" aria-label="${pct(r)} of the budget" style="--w:${Math.min(100, r * 100).toFixed(1)}%"><i></i></span></td>`,
    `<td class="n r" data-l="Budget">≤ ${esc(fmtNum(row.budget))} ${esc(row.budgetUnit)}</td>`,
    `<td data-l="CI gate">${gate}</td>`,
    `<td data-l="Source"><a href="${esc(row.source)}" rel="noopener">${esc(label)}</a></td>`,
    "</tr>",
  ].join("");
}

function table(rows, caption) {
  return [
    '<div class="table-wrap bench reveal">',
    "<table>",
    `<caption class="sr">${esc(caption)}</caption>`,
    '<thead><tr><th scope="col">Operation</th><th scope="col" class="r">Measured</th><th scope="col" class="r">Budget</th><th scope="col">CI gate</th><th scope="col">Source</th></tr></thead>',
    "<tbody>",
    ...rows.map(tableRow),
    "</tbody>",
    "</table>",
    "</div>",
  ].join("\n");
}

const bench = JSON.parse(read(join(SITE, "data", "bench.json")));
if (!Array.isArray(bench.rows) || !Array.isArray(bench.harsh)) throw new Error("bench.json needs rows[] and harsh[]");

const parts = [table(bench.rows, `Benchmark results, measured on ${bench.machine}`)];
parts.push(`<p class="fine">${esc(bench.method)} Measured on ${esc(bench.machine)}.</p>`);
if (bench.harsh.length) {
  parts.push('<h3 class="bench-h">Harsh conditions</h3>', table(bench.harsh, "Harsh-conditions benchmark results"));
} else {
  parts.push('<p class="bench-note">High-frequency stress rows (firehose transactions, keyed-list churn, stream backpressure, a soak run) are in flight. See the <a href="roadmap/">roadmap</a>.</p>');
}

const indexPath = join(SITE, "index.html");
let html = read(indexPath);
html = replaceRegion(html, "numbers", parts.join("\n"), "    ");
const push = `<button class="btn btn-primary" type="button" data-push-it${bench.stressScreen ? "" : " hidden"}>Push it</button>`;
html = replaceRegion(html, "pushit", push, "          ");
console.log(writeIfChanged(indexPath, html) ? "build-numbers: updated site/index.html" : "build-numbers: up to date");
