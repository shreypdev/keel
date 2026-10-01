#!/usr/bin/env node
// Renders the benchmark cards of the landing page (big number, one-line label, budget in mono) from site/data/bench.json, at build time.
// Writes between <!-- numbers:start --> and <!-- numbers:end --> in site/index.html, and shows the
// "Push it" button of the live demo only when bench.json says the stress screen exists.
//
//   node site/scripts/build-numbers.mjs
//
// bench.json: { machine, method, source, stressScreen, rows: [Row], harsh: [Row] } where a Row is
// { id, operation, value, unit, budget, budgetUnit, gate|null, source, floor?, measured? }. Units: ns, µs, ms, KB, MB, % for ceilings; /s, k/s, M/s for floor rows (throughput gates, `floor: true`).
//
// MEASURED ROWS. A row with `measured: { file, artifact }` is not typed by hand: its `value` is the
// `gzipped` size (KB = 1,000 bytes, one decimal) of the JSON line whose `artifact` matches in `file`
// (relative to the repository root; `scripts/wasm-size.sh --record` writes it, ADR-052), and this
// script writes it back into bench.json. The record's `budget` must be the row's. The same numbers
// fill every `<!--measured:NAME-->..<!--/measured-->` slot of the site's pages and of README.md
// (NAME is a key of SLOTS below), so a hand edit of any copy is undone here and fails CI's
// "generated files are up to date" check.
import { join } from "node:path";
import { SITE, read, writeIfChanged, replaceRegion, esc, htmlFiles } from "./lib.mjs";

const ROOT = join(SITE, "..");
/** Slot name -> the record file and artefact whose gzipped size it shows. */
const SLOTS = {
  "web-size": { file: "bench/results/web-size.jsonl", artifact: "web/hello-wasm" },
  "web-runtime-js": { file: "bench/results/web-size.jsonl", artifact: "web/hello-runtime-js" },
};

/** The JSON line of `artifact` in the record `file` (one JSON object per line). */
function recordOf({ file, artifact }) {
  const lines = read(join(ROOT, file)).split("\n").filter((l) => l.trim()).map((l) => JSON.parse(l));
  const line = lines.find((l) => l.artifact === artifact);
  if (!line) throw new Error(`${file} has no line for ${artifact}; run scripts/wasm-size.sh --record`);
  if (typeof line.gzipped !== "number") throw new Error(`${file}: ${artifact} was not measured (${line.error ?? "no gzipped size"}); run scripts/wasm-size.sh --record with the TypeScript runtime's node_modules installed`);
  return line;
}
/** A gzipped size in KB, one decimal, as the cards and the prose print it. */
const kb = (bytes) => Math.round(bytes / 100) / 10;

const FACTOR = { ns: 1, "µs": 1e3, ms: 1e6, KB: 1, MB: 1e3, "%": 1 };
// Throughput gates are floors: the measured rate must stay above them.
const RATE = { "/s": 1, "k/s": 1e3, "M/s": 1e6 };
const fmtNum = (n) => n.toLocaleString("en-US", { maximumFractionDigits: 2 });
const pct = (r) => (r >= 0.1 ? Math.round(r * 100) + "%" : (r * 100).toFixed(1) + "%");

function ratio(row) {
  const table = row.floor ? RATE : FACTOR;
  const a = table[row.unit], b = table[row.budgetUnit];
  if (!a || !b || !(row.budget > 0)) throw new Error(`bench row ${row.id}: unknown unit or budget`);
  // A ceiling row reports how much of its budget it uses; a floor row (a throughput gate) how much
  // of the measured rate the gate is, so the meter fills as the margin shrinks in both cases.
  return row.floor ? (row.budget * b) / (row.value * a) : (row.value * a) / (row.budget * b);
}

/** Column spans (of 12) for n cards: rows of 3, then a last row of 4 (seven rows make 3 + 4). */
function spans(n) {
  if (n >= 7) return Array.from({ length: n }, (_, i) => (i < n - 4 ? 4 : 3));
  if (n % 3 === 0) return Array(n).fill(4);
  if (n % 2 === 0) return Array(n).fill(6);
  return Array(n).fill(4);
}

function card(row, span) {
  const r = ratio(row);
  const count = Number.isInteger(row.value) ? 0 : (String(row.value).split(".")[1] || "").length;
  return [
    `<article class="stat${span === 4 ? " w4" : ""} reveal" data-bar>`,
    `<a class="lbl" href="${esc(row.source)}" rel="noopener">${esc(row.operation)}</a>`,
    `<div class="val"><span data-count="${row.value}" data-dec="${count}" data-final="${esc(fmtNum(row.value))}">${esc(fmtNum(row.value))}</span><small>${esc(row.unit)}</small></div>`,
    `<div class="meter"><div class="meter-track"><span class="meter-fill" style="--w:${Math.min(100, r * 100).toFixed(1)}%"></span></div><div class="meter-cap">${row.floor ? `<span><b>${fmtNum((row.value * RATE[row.unit]) / (row.budget * RATE[row.budgetUnit]))}×</b> the gate</span><span>gate ≥ ${esc(fmtNum(row.budget))} ${esc(row.budgetUnit)}</span>` : `<span><b>${pct(r)}</b> of budget</span><span>budget ≤ ${esc(fmtNum(row.budget))} ${esc(row.budgetUnit)}</span>`}</div></div>`,
    "</article>",
  ].join("");
}

function grid(rows, label) {
  const sp = spans(rows.length);
  return [`<div class="stats" role="group" aria-label="${esc(label)}">`, ...rows.map((row, i) => card(row, sp[i])), "</div>"].join("\n");
}

const benchPath = join(SITE, "data", "bench.json");
const bench = JSON.parse(read(benchPath));
if (!Array.isArray(bench.rows) || !Array.isArray(bench.harsh)) throw new Error("bench.json needs rows[] and harsh[]");

for (const row of [...bench.rows, ...bench.harsh].filter((r) => r.measured)) {
  const line = recordOf(row.measured);
  if (row.unit !== "KB" || row.budgetUnit !== "KB") throw new Error(`bench row ${row.id}: a measured row is in KB`);
  if (line.budget !== row.budget * 1000) throw new Error(`bench row ${row.id}: budget ${row.budget} KB, but ${row.measured.file} says ${line.budget} bytes`);
  row.value = kb(line.gzipped);
}
if (writeIfChanged(benchPath, JSON.stringify(bench, null, 2) + "\n")) console.log("build-numbers: updated site/data/bench.json from the measured records");

// The prose copies of the measured numbers.
const slotText = Object.fromEntries(Object.entries(SLOTS).map(([name, at]) => [name, `${kb(recordOf(at).gzipped)} KB`]));
const SLOT = /<!--measured:([a-z0-9-]+)-->[^<]*<!--\/measured-->/g;
for (const file of [...htmlFiles(SITE), join(ROOT, "README.md")]) {
  const text = read(file);
  if (!text.includes("<!--measured:")) continue;
  const filled = text.replace(SLOT, (_, name) => {
    if (!(name in slotText)) throw new Error(`${file}: unknown measured slot ${name}`);
    return `<!--measured:${name}-->${slotText[name]}<!--/measured-->`;
  });
  if (writeIfChanged(file, filled)) console.log(`build-numbers: updated ${file.slice(ROOT.length + 1)}`);
}

const parts = [grid(bench.rows, `Benchmark results, measured on ${bench.machine}`)];
if (bench.harsh.length) parts.push('<h3 class="stats-h">Harsh conditions</h3>', grid(bench.harsh, "Harsh-conditions benchmark results"));

parts.push(`<details class="measured"><summary>How these are measured</summary><p>${esc(bench.method)} Measured on ${esc(bench.machine)}. Device-measured rows are not claimed here; they are on the <a href="roadmap/">roadmap</a>.</p></details>`);

const indexPath = join(SITE, "index.html");
let html = read(indexPath);
html = replaceRegion(html, "numbers", parts.join("\n"), "    ");
const push = `<button class="btn btn-primary" type="button" data-push-it${bench.stressScreen ? "" : " hidden"}>Push it</button>`;
html = replaceRegion(html, "pushit", push, "          ");
console.log(writeIfChanged(indexPath, html) ? "build-numbers: updated site/index.html" : "build-numbers: up to date");
