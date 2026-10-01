#!/usr/bin/env node
// Writes site/docs/errors.html: one anchored section per macro diagnostic code (#E0001 ...).
//
// Sources, all in the repository, nothing typed by hand:
//   * docs/SPEC.md section 12 (the catalogue: code and trigger),
//   * the doc table in crates/<name>-macros/src/impl_/diag.rs (a short meaning per code),
//   * the compile-fail goldens crates/<name>-macros/tests/ui/*.stderr (the real messages: what, why, fix).
// Codes that no golden covers (they come from bindgen, rustc or the runtime) show the catalogue entry only.
//
//   node site/scripts/build-errors.mjs
import { readdirSync, existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { SITE, ORIGIN, read, writeIfChanged, replaceRegion, esc } from "./lib.mjs";

const REPO = resolve(SITE, "..");
const macros = readdirSync(join(REPO, "crates")).find((d) => d.endsWith("-macros"));
if (!macros) throw new Error("no crates/*-macros directory");
const MAX_ROWS = 6;

// ---- the catalogue (SPEC section 12)
const spec = read(join(REPO, "docs/SPEC.md"));
const sec = spec.slice(spec.indexOf("## 12. Diagnostics"), spec.indexOf("## 13."));
const trigger = new Map();
for (const m of sec.matchAll(/^\| (E\d{4}) \| (.+) \|$/gm)) trigger.set(m[1], m[2]);

// ---- short meanings (diag.rs doc table)
const meaning = new Map();
const diag = read(join(REPO, "crates", macros, "src/impl_/diag.rs"));
for (const m of diag.matchAll(/^\/\/\/ \| (E\d{4}) \| (.+) \|$/gm)) meaning.set(m[1], m[2].replace(/\s*\(addition\)\s*$/, "").replace(/;? ?\(a runtime message.*$/, ""));

// ---- the real messages (compile-fail goldens)
const dir = join(REPO, "crates", macros, "tests/ui");
const seen = new Map();
let brand = "";
for (const f of readdirSync(dir).filter((x) => x.endsWith(".stderr")).sort()) {
  const text = read(join(dir, f));
  for (const m of text.matchAll(/error: error\[\w+::(E\d{4})\]: (.+)\n((?:[ \t]+= (?:note|help|docs): .*\n)+)/g)) {
    brand ||= /error\[(\w+)::/.exec(m[0])?.[1] ?? "";
    const note = /= note: (.*)/.exec(m[3])?.[1], help = /= help: (.*)/.exec(m[3])?.[1];
    if (!note || !help) continue;
    const rows = seen.get(m[1]) ?? [];
    if (!rows.some((r) => r.why === note)) rows.push({ what: m[2], why: note, fix: help });
    seen.set(m[1], rows);
  }
}

const FAMILIES = [
  ["types-and-shapes", "Types and shapes", (n) => n <= 8],
  ["errors-and-stores", "Errors and stores", (n) => n >= 10 && n <= 13],
  ["methods", "Methods", (n) => n >= 20 && n <= 22],
  ["ports", "Ports", (n) => n >= 30 && n <= 33],
  ["queries-and-mutations", "Queries and mutations", (n) => n >= 40 && n <= 42],
  ["names", "Names in the generators", (n) => n >= 50 && n <= 52],
  ["type-identity", "Type identity and the schema", (n) => n >= 60],
];

/** `code` spans of a message to <code>, everything else escaped. */
const md = (t) => t.split("`").map((part, i) => (i % 2 ? `<code>${esc(part)}</code>` : esc(part))).join("");
const codes = [...trigger.keys()].sort();
const sample = seen.get("E0004")?.[0];

const out = [];
out.push('<p class="crumb">Reference</p>', "<h1>Error codes</h1>");
out.push('<p class="lede">Every compile error the macros raise has a stable code, a fixed shape and a fix. The link at the end of each message lands on its code below.</p>');
if (sample) out.push(
  '<h2 id="how-to-read-one">How to read one</h2>',
  "<p>A diagnostic says what is wrong, why the rule exists and how to fix it. This one is verbatim from the macro test suite:</p>",
  `<div class="code"><div class="code-bar"><span><span class="lang">text</span></span></div><pre><code>error[${brand}::E0004]: ${esc(sample.what)}\n  = note: ${esc(sample.why)}\n  = help: ${esc(sample.fix)}\n  = docs: ${ORIGIN}docs/errors.html#E0004</code></pre></div>`,
);
out.push('<h2 id="all-codes">All codes</h2>', '<div class="table-wrap"><table><thead><tr><th>Code</th><th>Meaning</th></tr></thead><tbody>',
  ...codes.map((c) => `<tr><td><a href="#${c}"><code>${c}</code></a></td><td>${md(meaning.get(c) ?? String(trigger.get(c)).split(/[;(]/)[0].trim())}</td></tr>`), "</tbody></table></div>");

const toc = [["how-to-read-one", "How to read one"], ["all-codes", "All codes"]];
for (const [id, title, test] of FAMILIES) {
  const list = codes.filter((c) => test(+c.slice(1)));
  if (!list.length) continue;
  toc.push([id, title]);
  out.push(`<h2 id="${id}">${esc(title)}<a class="anchor" href="#${id}" aria-label="Link to this section">#</a></h2>`);
  for (const c of list) {
    const rows = (seen.get(c) ?? []).slice(0, MAX_ROWS);
    out.push(`<h3 id="${c}"><code>${c}</code> ${md(meaning.get(c) ?? String(trigger.get(c)).split(/[;(]/)[0].trim())}<a class="anchor" href="#${c}" aria-label="Link to ${c}">#</a></h3>`);
    out.push(`<p>${md(trigger.get(c))}</p>`);
    if (rows.length) out.push('<div class="table-wrap"><table><thead><tr><th>What you see</th><th>Why</th><th>Fix</th></tr></thead><tbody>', ...rows.map((r) => `<tr><td>${md(r.what)}</td><td>${md(r.why)}</td><td>${md(r.fix)}</td></tr>`), "</tbody></table></div>");
    else out.push('<p class="tok-dim">No compile-fail example: this code comes from the generators or the runtime, not from the macros.</p>');
  }
}
out.push(`<p class="tok-dim">Generated by <code>site/scripts/build-errors.mjs</code> from <code>docs/SPEC.md</code> section 12 and the compile-fail tests of the macros crate. To change a message, change the macro; to change the catalogue, change the specification.</p>`);

const file = join(SITE, "docs", "errors.html");
let html = read(file);
html = replaceRegion(html, "errors", out.join("\n"), "");
html = replaceRegion(html, "errors-toc", `<ul>${toc.map(([id, t]) => `<li><a href="#${id}">${esc(t)}</a></li>`).join("")}</ul>`, "");
console.log(`build-errors: ${codes.length} codes, ${[...seen.values()].reduce((n, r) => n + r.length, 0)} real messages; ` + (writeIfChanged(file, html) ? "updated" : "up to date"));
