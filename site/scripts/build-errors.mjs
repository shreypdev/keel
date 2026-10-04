#!/usr/bin/env node
// Writes site/docs/errors.html: one anchored section per diagnostic code (#E0001 ... #C0014).
//
// Sources, all in the repository, nothing typed by hand:
//   * docs/SPEC.md section 12 (the catalogue: code, who raises it, trigger),
//   * the code table in crates/<name>-macros/src/impl_/diag.rs (a short meaning per E code) and the
//     variants of `Code` in crates/<name>-cli/src/error.rs (the meaning of a C code),
//   * the real messages: the compile-fail goldens crates/<name>-macros/tests/ui/*.stderr and the message
//     goldens crates/*/tests/golden/diagnostics/*.txt (schema validation, the runtime), each message
//     with its what, why and fix.
// A code of the catalogue with no real message is an error: the page never shows a bare entry. The
// audit that keeps the sources honest is crates/<name>-macros/tests/catalogue.rs.
//
//   node site/scripts/build-errors.mjs
import { readdirSync, existsSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { SITE, ORIGIN, read, writeIfChanged, replaceRegion, esc } from "./lib.mjs";

const REPO = resolve(SITE, "..");
const crateDir = (suffix) => readdirSync(join(REPO, "crates")).find((d) => d.endsWith(suffix) && existsSync(join(REPO, "crates", d, "Cargo.toml")));
const macros = crateDir("-macros");
const cli = crateDir("-cli");
if (!macros) throw new Error("no crates/*-macros directory");
const MAX_ROWS = 6;

// ---- the catalogue (SPEC section 12): `| E0001 | raised by | trigger |`
const spec = read(join(REPO, "docs/SPEC.md"));
const sec = spec.slice(spec.indexOf("## 12. Diagnostics"), spec.indexOf("## 13."));
const trigger = new Map();
const raisedBy = new Map();
// Whitespace-tolerant, so a reflowed (padded) table reads the same.
for (const m of sec.matchAll(/^\s*\|\s*([EC]\d{4})\s*\|\s*([^|]+?)\s*\|\s*(.+?)\s*\|\s*$/gm)) {
  trigger.set(m[1], m[3]);
  raisedBy.set(m[1], m[2]);
}

// ---- short meanings: the doc table of diag.rs (E codes) and the doc comments of `Code` (C codes)
const meaning = new Map();
const diag = read(join(REPO, "crates", macros, "src/impl_/diag.rs"));
for (const m of diag.matchAll(/^\/\/\/ \| (E\d{4}) \| (.+) \|$/gm)) meaning.set(m[1], m[2].replace(/\s*\((?:schema validation; )?addition\)\s*$/, ""));
if (cli) {
  const errorRs = read(join(REPO, "crates", cli, "src/error.rs"));
  for (const m of errorRs.matchAll(/^\s*\/\/\/ `(C\d{4})`: (.+?)\.?$/gm)) meaning.set(m[1], m[2]);
}

// ---- the real messages
/** The Undra diagnostics in `text`: the first line holds `error[<brand>::E0001]: what` (after `error: `, `error[E0277]: ` or
 *  `evaluation panicked: `), the next lines `= note:`, `= help:` and `= docs:`. */
function messagesOf(text) {
  const lines = text.split("\n");
  const found = [];
  for (let i = 0; i < lines.length; i++) {
    const m = /(?:^|:\s)error\[(\w+)::([EC]\d{4})\]: (.+)$/.exec(lines[i]);
    if (!m) continue;
    const parts = { note: [], help: [] };
    let kind = "";
    for (let j = i + 1; j < lines.length; j++) {
      const line = /^\s+= (note|help|docs): (.*)$/.exec(lines[j]);
      if (line) {
        kind = line[1];
        if (kind === "docs") break;
        parts[kind].push(line[2]);
      } else if (kind === "help" && /^\s{10,}\S/.test(lines[j])) parts.help.push(lines[j].trim()); // a CLI fix spans lines
      else break;
    }
    if (parts.note.length && parts.help.length) found.push({ brand: m[1], code: m[2], what: m[3], why: parts.note.join(" "), fix: parts.help.join(" ") });
  }
  return found;
}

const seen = new Map();
const nativeGolden = new Map(); // a code whose message is rustc's own: the ui test named after it
let brand = "";
const add = (message) => {
  brand ||= message.brand;
  const rows = seen.get(message.code) ?? [];
  if (!rows.some((r) => r.why === message.why && r.what === message.what)) rows.push({ what: message.what, why: message.why, fix: message.fix });
  seen.set(message.code, rows);
};
const ui = join(REPO, "crates", macros, "tests/ui");
// The compiler's own message stands in for a branded one only for a code SPEC says rustc raises,
// and only if it quotes the name the macro gave the thing rustc reports (how a reader finds the code).
const compilerRaised = (c) => /^rustc\b/.test(raisedBy.get(c) ?? "");
for (const f of readdirSync(ui).filter((x) => x.endsWith(".stderr")).sort()) {
  const text = read(join(ui, f));
  for (const message of messagesOf(text)) add(message);
  const named = /^(e\d{4})_/.exec(f);
  const c = named?.[1].toUpperCase();
  if (c && compilerRaised(c)) {
    if (!text.includes(`_error_${c}_`)) throw new Error(`${f}: the compiler's message of ${c} does not name the assertion that leads to the code`);
    nativeGolden.set(c, { file: f, text });
  }
}
/** Every `crates/<crate>/tests/golden/diagnostics/*.txt`: messages that no macro test can show. */
const goldenDirs = readdirSync(join(REPO, "crates"))
  .map((c) => join(REPO, "crates", c, "tests/golden/diagnostics"))
  .filter((d) => existsSync(d) && statSync(d).isDirectory());
for (const d of goldenDirs) for (const f of readdirSync(d).filter((x) => x.endsWith(".txt")).sort()) for (const message of messagesOf(read(join(d, f)))) add(message);

// ---- the page
const FAMILIES = [
  ["types-and-shapes", "Types and shapes", (c) => c[0] === "E" && +c.slice(1) <= 8],
  ["errors-and-stores", "Errors and stores", (c) => c[0] === "E" && +c.slice(1) >= 10 && +c.slice(1) <= 13],
  ["methods", "Methods", (c) => c[0] === "E" && +c.slice(1) >= 20 && +c.slice(1) <= 22],
  ["ports", "Ports", (c) => c[0] === "E" && +c.slice(1) >= 30 && +c.slice(1) <= 33],
  ["queries-and-mutations", "Queries and mutations", (c) => c[0] === "E" && +c.slice(1) >= 40 && +c.slice(1) <= 42],
  ["names", "Names in the generators", (c) => c[0] === "E" && +c.slice(1) >= 50 && +c.slice(1) <= 52],
  ["type-identity", "Type identity and the schema", (c) => c[0] === "E" && +c.slice(1) >= 60],
  ["command-line", "The command line", (c) => c[0] === "C"],
];

/** `code` spans of a message to <code>, everything else escaped. */
const md = (t) => t.split("`").map((part, i) => (i % 2 ? `<code>${esc(part)}</code>` : esc(part))).join("");
const codes = [...trigger.keys()].sort();
const short = (c) => meaning.get(c) ?? String(trigger.get(c)).split(/[;(]/)[0].trim();

// The catalogue as parsed must hold every code the code table of the macros and the command line's
// `Code` know: a SPEC table the regex above cannot read must fail here, not drop codes from the page.
const known = [...meaning.keys()];
const unread = known.filter((c) => !trigger.has(c));
if (unread.length) throw new Error(`codes in diag.rs or error.rs that are not read from SPEC section 12: ${unread.join(", ")}`);

// Every code of the catalogue shows a real message: a branded one, or the compiler's own.
const bare = codes.filter((c) => c[0] === "E" && !seen.has(c) && !nativeGolden.has(c));
if (bare.length) throw new Error(`no real message (compile-fail golden or crates/*/tests/golden/diagnostics/<code>.txt) for ${bare.join(", ")}`);

const sample = seen.get("E0004")?.[0];

/** The compiler's own message, as a user's file would show it. */
const native = (c) => {
  const { file, text } = nativeGolden.get(c);
  const clean = text
    .replace(/\$DIR\/tests\/ui\/\w+\.rs/g, "src/lib.rs")
    .replace(new RegExp(`tests/ui/${file.replace(/\.stderr$/, "\\.rs")}`, "g"), "src/lib.rs")
    .replace(/\n+$/, "");
  return `<p>The compiler reports this one itself, so it carries no Undra code. It is pointed at the method, and the last note names the assertion the macro emitted, <code>${esc(/_undra_error_E\d{4}_\w+/.exec(clean)?.[0] ?? c)}</code>: the code to look up.</p><div class="code"><div class="code-bar"><span><span class="lang">text</span></span></div><pre><code>${esc(clean)}</code></pre></div>`;
};

const out = [];
out.push('<p class="crumb">Reference</p>', "<h1>Error codes</h1>");
out.push('<p class="lede">Every diagnostic Undra raises, from a macro at compile time, from schema validation, from the runtime or from the <code>undra</code> command, has a stable code, a fixed shape and a fix. The link at the end of each message lands on its code below.</p>');
if (sample) out.push(
  '<h2 id="how-to-read-one">How to read one</h2>',
  "<p>A diagnostic says what is wrong, why the rule exists and how to fix it. This one is verbatim from the macro test suite:</p>",
  `<div class="code"><div class="code-bar"><span><span class="lang">text</span></span></div><pre><code>error[${brand}::E0004]: ${esc(sample.what)}\n  = note: ${esc(sample.why)}\n  = help: ${esc(sample.fix)}\n  = docs: ${ORIGIN}docs/errors.html#E0004</code></pre></div>`,
);
out.push('<h2 id="all-codes">All codes</h2>', '<div class="table-wrap"><table><thead><tr><th>Code</th><th>Meaning</th></tr></thead><tbody>',
  ...codes.map((c) => `<tr><td><a href="#${c}"><code>${c}</code></a></td><td>${md(short(c))}</td></tr>`), "</tbody></table></div>");

const toc = [["how-to-read-one", "How to read one"], ["all-codes", "All codes"]];
const placed = new Set();
for (const [id, title, test] of FAMILIES) {
  const list = codes.filter(test);
  if (!list.length) continue;
  list.forEach((c) => placed.add(c));
  toc.push([id, title]);
  out.push(`<h2 id="${id}">${esc(title)}<a class="anchor" href="#${id}" aria-label="Link to this section">#</a></h2>`);
  for (const c of list) {
    const rows = (seen.get(c) ?? []).slice(0, MAX_ROWS);
    out.push(`<h3 id="${c}"><code>${c}</code> ${md(short(c))}<a class="anchor" href="#${c}" aria-label="Link to ${c}">#</a></h3>`);
    out.push(`<p class="tok-dim">Raised by ${md(raisedBy.get(c))}.</p>`);
    out.push(`<p>${md(trigger.get(c))}</p>`);
    if (rows.length) out.push('<div class="table-wrap"><table><thead><tr><th>What you see</th><th>Why</th><th>Fix</th></tr></thead><tbody>', ...rows.map((r) => `<tr><td>${md(r.what)}</td><td>${md(r.why)}</td><td>${md(r.fix)}</td></tr>`), "</tbody></table></div>");
    else if (nativeGolden.has(c)) out.push(native(c));
    else out.push('<p class="tok-dim">The command prints this diagnostic with the details of the failure that caused it.</p>');
  }
}
const lost = codes.filter((c) => !placed.has(c));
if (lost.length) throw new Error(`codes outside every family: ${lost.join(", ")}`);

// ---- the JavaScript runtime's messages: T0001 ..., from the one table of the runtime (ADR-057 D1, D8)
const runtimeMessages = read(join(REPO, "runtimes/ts/@undra/runtime/src/messages.ts"));
const table = runtimeMessages.slice(runtimeMessages.indexOf("const MESSAGES"), runtimeMessages.indexOf("\n};", runtimeMessages.indexOf("const MESSAGES")));
const tRows = [];
for (const line of table.split("\n").slice(1)) {
  const m = /^ {2}(\d+): (null|"(?:[^"\\]|\\.)*"),(?: \/\/ (.*))?$/.exec(line);
  if (!m) throw new Error(`runtimes/ts/@undra/runtime/src/messages.ts: a row the page cannot read: ${line}`);
  if (m[2] === "null") continue;
  if (!m[3]) throw new Error(`runtimes/ts/@undra/runtime/src/messages.ts: code ${m[1]} says no \`// where it is raised\``);
  tRows.push({ id: `T${m[1].padStart(4, "0")}`, text: JSON.parse(m[2]), where: m[3] });
}
// The wire failures: the cases of `describe`, its template literal written with `{field}` for each value.
const wireRows = [...runtimeMessages.matchAll(/case "(\w+)":\s*\n\s*return ([`"])(.*)\2;/g)].map((m) => ({
  id: `wire-${m[1]}`,
  code: m[1],
  text: m[3].replace(/\$\{d\.\w+ === 1 \? "" : "s"\}/g, "(s)").replace(/\$\{d\.(\w+)(?:[^}]*)\}/g, (_x, field) => `{${field}}`).replace(/\$\{[^}]*\}/g, "{…}"),
}));
if (tRows.length < 100 || wireRows.length < 11) throw new Error(`the runtime's messages are not read from messages.ts (${tRows.length} codes, ${wireRows.length} wire failures)`);
toc.push(["runtime-messages", "Runtime messages"]);
out.push(
  `<h2 id="runtime-messages">Runtime messages<a class="anchor" href="#runtime-messages" aria-label="Link to this section">#</a></h2>`,
  "<p>The JavaScript runtime (<code>@undra/runtime</code>) throws and logs these. Its <strong>production build</strong> (what <code>vite build</code>, webpack in production mode and anything else that asks for neither the <code>development</code> nor the <code>react-native</code> condition resolve) says a code, the values and a link instead of a sentence, so a page does not carry the prose of an error whose class, <code>kind</code> and fields it already has: <code>T0017: callSync, remote; see " + esc(ORIGIN) + "docs/errors.html#T0017</code>. Its <strong>development build</strong> (Vite's dev server, Vitest, React Native) says the sentence. Both throw the same classes with the same <code>kind</code> and fields; only <code>message</code> differs, so nothing a program can branch on changes. Text the runtime hands on as data, the field of a port's typed error (which the core receives) and a WebSocket close frame's reason (which the peer receives), is not a message: it is the same sentence in both builds. Codes are never reused.</p>",
  `<h3 id="runtime-messages-table">The T codes (${tRows.length})<a class="anchor" href="#runtime-messages-table" aria-label="Link to this section">#</a></h3>`,
  "<p>The sentence is what the development build says; <code>{0}</code>, <code>{1}</code>, ... are the values the production message lists, in that order.</p>",
  '<div class="table-wrap"><table><thead><tr><th>Code</th><th>The development build says</th><th>Raised by</th></tr></thead><tbody>',
  ...tRows.map((r) => `<tr id="${r.id}"><td><a href="#${r.id}"><code>${r.id}</code></a></td><td>${md(r.text)}</td><td>${md(r.where)}</td></tr>`),
  "</tbody></table></div>",
  `<h3 id="wire-failures">Wire failures<a class="anchor" href="#wire-failures" aria-label="Link to this section">#</a></h3>`,
  "<p>A <code>WireError</code> says its code and fields in a production build (<code>wire: code=unexpected_eof at=12 needed=3; see link</code>); the development build says the sentence below, with the values in braces.</p>",
  '<div class="table-wrap"><table><thead><tr><th>Code</th><th>The development build says</th></tr></thead><tbody>',
  ...wireRows.map((r) => `<tr id="${r.id}"><td><a href="#${r.id}"><code>${r.code}</code></a></td><td>${md(r.text)}</td></tr>`),
  "</tbody></table></div>",
);
out.push(`<p class="tok-dim">Generated by <code>site/scripts/build-errors.mjs</code> from <code>docs/SPEC.md</code> section 12, the compile-fail tests of the macros crate and the message goldens of the other crates. To change a message, change the code that raises it and regenerate its golden; to change the catalogue, change the specification.</p>`);

const file = join(SITE, "docs", "errors.html");
let html = read(file);
html = replaceRegion(html, "errors", out.join("\n"), "");
html = replaceRegion(html, "errors-toc", `<ul>${toc.map(([id, t]) => `<li><a href="#${id}">${esc(t)}</a></li>`).join("")}</ul>`, "");
console.log(`build-errors: ${codes.length} codes, ${[...seen.values()].reduce((n, r) => n + r.length, 0)} real messages; ` + (writeIfChanged(file, html) ? "updated" : "up to date"));
