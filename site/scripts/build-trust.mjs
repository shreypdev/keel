#!/usr/bin/env node
// Renders the landing page's two "receipts" cards (the test counts and the contract-scenario grid) and fills
// the numbers into the prose that repeats them, so no count is typed by hand:
//
//   node site/scripts/build-trust.mjs
//
// Inputs: site/data/tests.json (the suite counts of the last merge and the number of passing cells) and
// contract-tests/scenarios.md (the scenarios: every `### S01 title` heading is one; "TypeScript only" in the
// heading means Swift and Kotlin do not run it). `cellsPassing` must equal the cells the scenarios define,
// so a new scenario that nobody has run on every platform fails the build instead of showing as passing.
//
// Writes between `<!-- trust:start -->` and `<!-- trust:end -->` in site/index.html, and every
// `<!--trust:NAME-->..<!--/trust-->` slot of the site's pages and of README.md (NAME is a key of `slots`
// below), so a hand edit of any copy is undone here and fails CI's "generated files are up to date" check.
import { join } from "node:path";
import { SITE, read, writeIfChanged, replaceRegion, esc, htmlFiles } from "./lib.mjs";

const ROOT = join(SITE, "..");
const PLATFORMS = ["Swift", "Kotlin", "TS"];
const fmt = (n) => n.toLocaleString("en-US");

const tests = JSON.parse(read(join(SITE, "data", "tests.json")));
if (!Array.isArray(tests.suites) || !tests.suites.length || !Number.isInteger(tests.cellsPassing)) throw new Error("tests.json needs suites[] and cellsPassing");

// The scenarios, in order.
const scenarios = [...read(join(ROOT, "contract-tests", "scenarios.md")).matchAll(/^### (S\d{2}) (.+)$/gm)].map((m) => ({
  id: m[1],
  title: m[2].replace(/\s*\([^)]*\)\s*$/, ""),
  webOnly: /TypeScript only/.test(m[2]),
}));
scenarios.forEach((s, i) => {
  if (s.id !== `S${String(i + 1).padStart(2, "0")}`) throw new Error(`contract-tests/scenarios.md: ${s.id} is out of order (expected S${String(i + 1).padStart(2, "0")})`);
});
const runs = (s, platform) => !s.webOnly || platform === "TS";
const cells = scenarios.length * PLATFORMS.length - scenarios.filter((s) => s.webOnly).length * (PLATFORMS.length - 1);
if (cells !== tests.cellsPassing) throw new Error(`tests.json says ${tests.cellsPassing} cells pass, but contract-tests/scenarios.md defines ${cells} (${scenarios.length} scenarios on ${PLATFORMS.length} platforms); run the grid and update cellsPassing`);

const suites = [...tests.suites].sort((a, b) => b.count - a.count);
const total = suites.reduce((n, s) => n + s.count, 0);

const testsCard = [
  `<article class="tcard t-tests reveal" data-bar>`,
  `<h3><a href="https://github.com/shreypdev/undra#why-you-can-trust-it" rel="noopener">Tests</a></h3>`,
  `<div class="big"><span data-count="${total}" data-dec="0" data-final="${fmt(total)}">${fmt(total)}</span></div>`,
  `<div class="seg" role="img" aria-label="${esc(suites.map((s) => `${s.name} ${fmt(s.count)}`).join(", "))}">${suites.map((s) => `<i style="--w:${((s.count / total) * 100).toFixed(1)}%"></i>`).join("")}</div>`,
  `<ul class="seg-key" role="list">${suites.map((s) => `<li>${esc(s.name)}<b>${fmt(s.count)}</b></li>`).join("")}</ul>`,
  `</article>`,
].join("\n");

const rows = PLATFORMS.map((platform) => {
  const cellsOf = scenarios.map((s, i) => {
    const title = esc(`${s.id} ${s.title} (${platform})`);
    return runs(s, platform)
      ? `<i style="--i:${i}" title="${title}: pass"></i>`
      : `<i class="na" style="--i:${i}" title="${title}: not run, web only"></i>`;
  });
  return `<span class="r">${platform}</span>${cellsOf.join("")}`;
});
const webOnly = scenarios.filter((s) => s.webOnly).length;
const cellsCard = [
  `<article class="tcard t-cells reveal" data-bar data-d="1">`,
  `<h3><a href="https://github.com/shreypdev/undra/blob/main/contract-tests/scenarios.md" rel="noopener">${scenarios.length} scenarios on ${PLATFORMS.length} platforms</a></h3>`,
  `<div class="big">${cells}<small>/ ${cells} pass</small></div>`,
  `<div class="gridcells" style="--cols:${scenarios.length}" role="img" aria-label="${scenarios.length} scenarios by ${PLATFORMS.length} platforms, ${cells} cells, all passing${webOnly ? `; ${webOnly} scenario${webOnly > 1 ? "s" : ""} run on the web only` : ""}">`,
  rows.join("\n"),
  `</div>`,
  `</article>`,
].join("\n");

// The numbers the prose repeats.
const slots = {
  "tests-total": fmt(total),
  scenarios: String(scenarios.length),
  cells: String(cells),
  "cells-of": `${cells} of ${cells}`,
  ...Object.fromEntries(tests.suites.map((s) => [`tests-${s.slug}`, fmt(s.count)])),
};
const SLOT = /<!--trust:([a-z0-9-]+)-->[^<]*<!--\/trust-->/g;
for (const file of [...htmlFiles(SITE), join(ROOT, "README.md")]) {
  const text = read(file);
  if (!text.includes("<!--trust:") && !text.includes("<!--/trust-->")) continue;
  // As build-numbers.mjs: every opener and closer must belong to a whole slot with plain text inside.
  const whole = [...text.matchAll(SLOT)].length;
  const openers = text.split("<!--trust:").length - 1;
  const closers = text.split("<!--/trust-->").length - 1;
  if (openers !== whole || closers !== whole) {
    throw new Error(`${file}: ${openers} <!--trust:NAME--> and ${closers} <!--/trust--> markers, but only ${whole} whole slots; a slot holds plain text only: <!--trust:cells-of-->95 of 95<!--/trust-->`);
  }
  const filled = text.replace(SLOT, (_, name) => {
    if (!(name in slots)) throw new Error(`${file}: unknown trust slot ${name} (known: ${Object.keys(slots).join(", ")})`);
    return `<!--trust:${name}-->${slots[name]}<!--/trust-->`;
  });
  if (writeIfChanged(file, filled)) console.log(`build-trust: updated ${file.slice(ROOT.length + 1)}`);
}

const indexPath = join(SITE, "index.html");
const html = replaceRegion(read(indexPath), "trust", `${testsCard}\n${cellsCard}`, "      ");
console.log(writeIfChanged(indexPath, html) ? "build-trust: updated site/index.html" : "build-trust: up to date");
