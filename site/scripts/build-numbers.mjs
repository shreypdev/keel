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
// MEASURED ROWS. A row with a `measured` object is not typed by hand: this script reads its `value`
// from a record file (paths are relative to the repository root) and writes it back into bench.json.
//   * { file, artifact, field? }: a size. `file` is a JSONL record (`scripts/wasm-size.sh --record`
//     writes the web one, ADR-052); the line whose `artifact` matches gives `field` (default
//     `gzipped`, bytes), shown in KB (1,000 bytes, one decimal). The record's `budget` (bytes) must be
//     the row's.
//   * { file, path }: a number of a whole-file JSON record (the harsh-conditions results of
//     bench/results/), `path` dotted ("result.per_sec"), converted to the row's unit (ns, µs, ms for a
//     time, /s, k/s, M/s for a rate, % as it is) and rounded to the row's `digits` decimals (default 1).
//     The row's `operation` may carry `{path|time}`, `{path|int}` or `{path|pct}` tokens (a value of the same
//     file, as a time with three significant figures, as a whole number, or as a signed percentage
//     with two decimals); they are filled in when the card is drawn, and the template stays in bench.json.
// The same size numbers fill every `<!--measured:NAME-->..<!--/measured-->` slot of the site's pages
// and of README.md (NAME is a key of SLOTS below, or `row-<id>` for the value and unit of a card of
// bench.json), so a hand edit of any copy is undone here and fails CI's "generated files are up to date" check.
import { join } from "node:path";
import { SITE, read, writeIfChanged, replaceRegion, esc, htmlFiles } from "./lib.mjs";

const ROOT = join(SITE, "..");
/** Slot name -> the record file and artefact whose gzipped size it shows. */
const SLOTS = {
  "web-size": { file: "bench/results/web-size.jsonl", artifact: "web/hello-wasm" },
  "web-runtime-js": { file: "bench/results/web-size.jsonl", artifact: "web/hello-runtime-js" },
  "android-size": { file: "bench/results/native-size.jsonl", artifact: "android/hello-arm64-v8a", field: "bytes" },
  "android-x86-size": { file: "bench/results/native-size.jsonl", artifact: "android/hello-x86_64", field: "bytes" },
  "ios-size": { file: "bench/results/native-size.jsonl", artifact: "ios/hello-arm64", field: "bytes" },
};

/** The JSON line of `artifact` in the record `file` (one JSON object per line), and its `field` in bytes. */
function recordOf({ file, artifact, field = "gzipped" }) {
  const lines = read(join(ROOT, file)).split("\n").filter((l) => l.trim()).map((l) => JSON.parse(l));
  const line = lines.find((l) => l.artifact === artifact);
  if (!line) throw new Error(`${file} has no line for ${artifact}; record it again (scripts/wasm-size.sh --record for the web, scripts/native-size.sh --record for Android)`);
  if (typeof line[field] !== "number") throw new Error(`${file}: ${artifact} was not measured (${line.error ?? `no ${field}`}); run scripts/wasm-size.sh --record with the TypeScript runtime's node_modules installed`);
  return { ...line, bytes: line[field] };
}
/** The value at a dotted `path` of a whole-file JSON record. */
function jsonAt(file, path) {
  const value = path.split(".").reduce((o, k) => (o == null ? undefined : o[k]), JSON.parse(read(join(ROOT, file))));
  if (typeof value !== "number") throw new Error(`${file} has no number at ${path}`);
  return value;
}
/** A time in nanoseconds as a card label prints it: three significant figures, in ns, µs or ms. */
function fmtTime(ns) {
  const [v, unit] = ns < 1e3 ? [ns, "ns"] : ns < 1e6 ? [ns / 1e3, "µs"] : [ns / 1e6, "ms"];
  return `${Number(v.toPrecision(3))} ${unit}`;
}
/** A size in KB, one decimal, as the cards and the prose print it. */
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

/** Column spans (of 12) for n cards: rows of 3, then a last row of 4 (seven rows make 3 + 4); a
 * multiple of four from eight on is rows of 4 (eight make 4 + 4). */
function spans(n) {
  if (n >= 8 && n % 4 === 0) return Array(n).fill(3);
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
    `<a class="lbl" href="${esc(row.source)}" rel="noopener">${esc(labels[row.id] ?? row.operation)}</a>`,
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

/** The label of each row, with the `{path|format}` tokens of a record-backed row filled in. */
const labels = {};
for (const row of [...bench.rows, ...bench.harsh].filter((r) => r.measured)) {
  const m = row.measured;
  if (m.path) {
    const per = (row.floor ? RATE : FACTOR)[row.unit];
    if (!per) throw new Error(`bench row ${row.id}: unknown unit ${row.unit}`);
    row.value = Number((jsonAt(m.file, m.path) / per).toFixed(row.digits ?? 1));
    labels[row.id] = row.operation.replace(/\{([\w.]+)\|(time|int|pct)\}/g, (_, path, format) => {
      const v = jsonAt(m.file, path);
      return format === "time" ? fmtTime(v) : format === "pct" ? `+${v.toFixed(2)} %` : fmtNum(Math.round(v));
    });
    continue;
  }
  const line = recordOf(m);
  if (row.unit !== "KB" || !(row.budgetUnit in FACTOR)) throw new Error(`bench row ${row.id}: a measured size is in KB`);
  if (line.budget !== Math.round(row.budget * FACTOR[row.budgetUnit] * 1000)) throw new Error(`bench row ${row.id}: budget ${row.budget} ${row.budgetUnit}, but ${m.file} says ${line.budget} bytes`);
  row.value = kb(line.bytes);
}
if (writeIfChanged(benchPath, JSON.stringify(bench, null, 2) + "\n")) console.log("build-numbers: updated site/data/bench.json from the measured records");

// The prose copies of the measured numbers: the sizes above, and `row-<id>` for the value of any card of
// bench.json (the landing page's diagram shows the core call and the change-set this way).
const slotText = Object.fromEntries(Object.entries(SLOTS).map(([name, at]) => [name, `${kb(recordOf(at).bytes)} KB`]));
for (const row of [...bench.rows, ...bench.harsh]) slotText[`row-${row.id}`] = `${fmtNum(row.value)} ${row.unit}`;
const SLOT = /<!--measured:([a-z0-9-]+)-->[^<]*<!--\/measured-->/g;
for (const file of [...htmlFiles(SITE), join(ROOT, "README.md")]) {
  const text = read(file);
  if (!text.includes("<!--measured:") && !text.includes("<!--/measured-->")) continue;
  // Every opener and closer must belong to a whole slot with plain text inside: a slot whose
  // number was edited into markup, or whose closer was mistyped, would otherwise be skipped by the
  // pattern and keep its hand-written number past CI's "generated files are up to date" check.
  const whole = [...text.matchAll(SLOT)].length;
  const openers = text.split("<!--measured:").length - 1;
  const closers = text.split("<!--/measured-->").length - 1;
  if (openers !== whole || closers !== whole) {
    throw new Error(`${file}: ${openers} <!--measured:NAME--> and ${closers} <!--/measured--> markers, but only ${whole} whole slots; a slot holds plain text only: <!--measured:NAME-->95.7 KB<!--/measured-->`);
  }
  const filled = text.replace(SLOT, (_, name) => {
    if (!(name in slotText)) throw new Error(`${file}: unknown measured slot ${name}`);
    return `<!--measured:${name}-->${slotText[name]}<!--/measured-->`;
  });
  if (writeIfChanged(file, filled)) console.log(`build-numbers: updated ${file.slice(ROOT.length + 1)}`);
}

const parts = [grid(bench.rows, `Benchmark results, measured on ${bench.machine}`)];
// The harsh-conditions cards sit behind a disclosure: the landing page leads with the boundary's cost.
if (bench.harsh.length) parts.push('<details class="reveal-code harsh"><summary>Harsh conditions</summary>', grid(bench.harsh, "Harsh-conditions benchmark results"), "</details>");

parts.push(`<details class="measured"><summary>How these are measured</summary><p>${esc(bench.method)} Measured on ${esc(bench.machine)}. Simulator, emulator and Chromium rows are in the <a href="https://github.com/shreypdev/undra/blob/main/bench/RESULTS.md#device-numbers-ios-android-web" rel="noopener">benchmark results</a>; real-phone rows are not claimed and are on the <a href="roadmap/">roadmap</a>.</p></details>`);

const indexPath = join(SITE, "index.html");
let html = read(indexPath);
html = replaceRegion(html, "numbers", parts.join("\n"), "    ");
const push = `<button class="btn btn-primary" type="button" data-push-it${bench.stressScreen ? "" : " hidden"}>Push it</button>`;
html = replaceRegion(html, "pushit", push, "          ");
console.log(writeIfChanged(indexPath, html) ? "build-numbers: updated site/index.html" : "build-numbers: up to date");
