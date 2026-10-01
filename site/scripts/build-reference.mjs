#!/usr/bin/env node
// Writes the Swift, Kotlin and TypeScript API reference: site/reference/{swift,kotlin,typescript}.html.
//
// Source: the committed bindings of the playground (examples/playground/generated/{swift,kotlin,ts}),
// the output of `undra bindgen -C examples/playground --docs`. The playground is the public example
// (constitution R10), so its generated records, enums, errors, objects, stores, queries and ports, with
// the doc comments of the Rust core, are the reference of what Undra emits for each language. Each file
// becomes one section: its declarations (decls.mjs: bodies, initializers and plumbing left out) behind a
// "show" toggle, and the names it declares. Nothing here is typed by hand; the page skeletons carry the
// head, the sidebar and the footer, this script fills what lies between the reference:start/end markers.
//
// A bindings change that does not rebuild these pages fails CI: the site workflow runs build-all.mjs and
// fails when anything under site/ changed.
//
//   node site/scripts/build-reference.mjs
import { existsSync } from "node:fs";
import { join, resolve, posix } from "node:path";
import { SITE, read, writeIfChanged, replaceRegion, esc } from "./lib.mjs";
import { declarations } from "./decls.mjs";

const REPO = resolve(SITE, "..");
const GENERATED = join(REPO, "examples/playground/generated");
const GITHUB = "https://github.com/shreypdev/undra";
const SOURCE = `${GITHUB}/blob/main/examples/playground/generated`;

/** The generated files in the order of the TypeScript barrel (index.ts), and what each holds. `ts` and `native` are the wording per language. */
const SECTIONS = [
  { id: "entry", title: "Entry point", stem: "index", only: "ts", blurb: { ts: "The package entry: it re-exports every file below." } },
  { id: "core", title: "The core", stem: "core", blurb: {
    swift: "The core's entry, named after its namespace: <code>load</code> starts the core and checks its schema hash, and <code>core</code> is the one every generated call uses unless it is given another.",
    kotlin: "The core's entry, named after its namespace: <code>load</code> starts the core and checks its schema hash, and <code>core</code> is the one every generated call uses unless it is given another. The internal <code>UndraCoreNative</code> holds the natives the core's library registers over JNI.",
    ts: "The core's entry, named after its namespace: <code>load</code> starts the core and checks its schema hash, and <code>core</code> is the one every generated call uses unless it is given another." } },
  { id: "types", title: "Types", stem: "types", blurb: {
    swift: "A struct for each record, an enum for each enum, with the wire codec beside it.",
    kotlin: "A data class for each record, an enum or sealed interface for each enum, each with its codec.",
    ts: "An interface for each record, a union for each enum, each with a codec." } },
  { id: "errors", title: "Errors", stem: "errors", blurb: {
    swift: "An enum for each <code>#[undra::error]</code>, conforming to <code>Error</code>.",
    kotlin: "A sealed class for each <code>#[undra::error]</code>, one subclass per variant.",
    ts: "An abstract class for each <code>#[undra::error]</code>, one subclass per variant." } },
  { id: "objects", title: "Objects", stem: "objects", blurb: {
    swift: "A class for each object, and a function for each free Rust function.",
    kotlin: "A class for each object, and a function for each free Rust function.",
    ts: "A class for each object, and an async function for each free Rust function." } },
  { id: "stores", title: "Stores", stem: "stores", blurb: {
    swift: "An <code>@Observable</code> class for each <code>#[undra::store]</code>: its signals, then its methods.",
    kotlin: "A class for each <code>#[undra::store]</code>: its signals as <code>StateFlow</code>, then its methods.",
    ts: "A class for each <code>#[undra::store]</code>: its signals as <code>Signal</code>, then its methods." } },
  { id: "ports", title: "Ports", stem: "ports", empty: true, blurb: {
    swift: "A protocol for each port the core declares, and an adapter that registers your implementation.",
    kotlin: "An interface for each port the core declares, and an adapter that registers your implementation.",
    ts: "An interface for each port the core declares, and an adapter that registers your implementation." } },
  { id: "queries", title: "Queries", stem: "queries", blurb: {
    swift: "A handle for each <code>#[undra::query]</code>, a function for each <code>#[undra::mutation]</code>.",
    kotlin: "A handle for each <code>#[undra::query]</code>, a function for each <code>#[undra::mutation]</code>.",
    ts: "A handle for each <code>#[undra::query]</code>, a function for each <code>#[undra::mutation]</code>." } },
  { id: "ids", title: "Wire ids", stem: "ids", blurb: {
    swift: "The stable identifiers of every call, the core's namespace and the schema hash its entry checks at load.",
    kotlin: "The stable identifiers of every call, the core's namespace and the schema hash its entry checks at load.",
    ts: "The stable identifiers of every call, the core's namespace and the schema hash its entry checks at load." } },
];

/** What a language's generated code is called on its page. `source` is the directory of the generated files, `manifest` the prefix in `.undra-generated`. */
const PLATFORMS = [
  { page: "swift", name: "Swift", lang: "swift", ext: ".swift", prefix: "swift/", overview: "api-swift.html",
    ports: "Your own port becomes a protocol that your app implements; the ten standard ports are implemented by <code>UndraRuntime</code>." },
  { page: "kotlin", name: "Kotlin", lang: "kotlin", ext: ".kt", prefix: "kotlin/", overview: "api-kotlin.html",
    ports: "Your own port becomes an interface that your app implements; the ten standard ports come with the runtime module." },
  { page: "typescript", name: "TypeScript", lang: "ts", ext: ".ts", prefix: "ts/", overview: "api-typescript.html",
    ports: "Your own port becomes an interface that your app implements; the ten standard ports are implemented by <code>@undra/runtime</code>." },
];

const manifest = read(join(GENERATED, ".undra-generated")).split("\n").filter(Boolean);
/** The schema hash a generated file states in its first line. */
const hashOf = (text) => /schema hash (0x[0-9a-f]{16})/.exec(text.split("\n", 1)[0])?.[1];
const hashes = new Set();

/** `<code>` for each name, in the order the file declares them. */
const chips = (names) => names.map((n) => `<code>${esc(n)}</code>`).join(" ");
/** Source text as HTML: only &, < and > need care inside a <pre>. */
const code = (text) => text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

const stats = [];
for (const p of PLATFORMS) {
  // the source files of this language, from the manifest: a file no section claims is an error, so a new kind of generated file shows up here
  const sources = manifest.filter((f) => f.startsWith(p.prefix) && f.endsWith(p.ext) && !/(?:^|\/)Package\.swift$/.test(f));
  const byStem = new Map(sources.map((f) => [posix.basename(f, p.ext).toLowerCase(), f]));
  const claimed = new Set();
  const html = [];
  const toc = [];
  const used = SECTIONS.filter((s) => !s.only || s.only === p.lang);
  let declared = 0;
  for (const sec of used) {
    const rel = byStem.get(sec.stem);
    if (!rel) throw new Error(`${p.page}: no generated ${sec.stem}${p.ext} in ${GENERATED}/.undra-generated`);
    claimed.add(rel);
    const text = read(join(GENERATED, rel));
    const hash = hashOf(text);
    if (!hash) throw new Error(`${rel}: no schema hash in its first line`);
    hashes.add(hash);
    const { text: decls, names } = declarations(text, p.lang);
    const lines = decls ? decls.trimEnd().split("\n").length : 0;
    declared += names.length;
    toc.push([sec.id, sec.title]);
    html.push(`<h2 id="${sec.id}">${esc(sec.title)}<a class="anchor" href="#${sec.id}" aria-label="Link to this section">#</a></h2>`);
    if (sec.empty && !lines) { // a file with no declarations: the playground has no port of its own
      html.push(`<p class="ref-blurb">The playground declares no ${sec.title.toLowerCase().replace(/s$/, "")} of its own, so <code>${esc(posix.basename(rel))}</code> holds none. ${p.ports} <a href="../docs/ports.html">Ports &amp; adapters</a> shows both.</p>`);
      continue;
    }
    const blurb = sec.blurb[p.lang] ?? sec.blurb.swift;
    html.push(`<p class="ref-blurb">${blurb}</p>`);
    if (names.length) html.push(`<p class="ref-names">${chips(names)}</p>`);
    html.push(
      `<details class="reveal-code ref"><summary>Show the declarations <span>${lines} lines</span></summary>`,
      `<div class="code"><div class="code-bar"><span><a href="${SOURCE}/${rel}" rel="noopener">${esc(posix.basename(rel))}</a></span></div><pre><code data-lang="${p.lang}">${code(decls)}</code></pre></div>`,
      "</details>",
    );
  }
  const stray = sources.filter((f) => !claimed.has(f));
  if (stray.length) throw new Error(`${p.page}: generated files no section shows: ${stray.join(", ")} (add them to SECTIONS in build-reference.mjs)`);
  stats.push({ p, html, toc, sections: used.length, declared });
}
if (hashes.size !== 1) throw new Error(`the generated files disagree about the schema hash: ${[...hashes].join(", ")} (run: undra bindgen -C examples/playground --docs)`);
const [hash] = hashes;

let changed = 0;
for (const { p, html, toc, declared } of stats) {
  const file = join(SITE, "reference", `${p.page}.html`);
  if (!existsSync(file)) throw new Error(`site/reference/${p.page}.html (the page skeleton) is missing`);
  const intro = [
    '<p class="crumb">API reference</p>',
    `<h1>${p.name} reference</h1>`,
    `<p class="lede">Every declaration <code>undra bindgen</code> generates in ${p.name} for the <a href="${GITHUB}/tree/main/examples/playground" rel="noopener">playground</a> (schema hash <code class="cw">${hash}</code>). <a href="../docs/${p.overview}">The API overview</a> explains how it is shaped.</p>`,
    '<p class="ref-note">Bodies, initializers, private and override members are left out, and a run of declarations that differ only by a number is folded to its ends. Each file name links to its complete source.</p>',
  ];
  const foot = `<p class="tok-dim">Generated by <code>site/scripts/build-reference.mjs</code> from <code>examples/playground/generated/${p.prefix.slice(0, -1)}</code>, with the declarations read by <code>site/scripts/decls.mjs</code>. To change this page, change the core's code or comments and run <code>undra bindgen -C examples/playground --docs</code>.</p>`;
  let page = read(file);
  page = replaceRegion(page, "reference", [...intro, ...html, foot].join("\n"), "");
  page = replaceRegion(page, "reference-toc", `<ul>${toc.map(([id, t]) => `<li><a href="#${id}">${esc(t)}</a></li>`).join("")}</ul>`, "");
  if (writeIfChanged(file, page)) changed++;
  console.log(`build-reference: ${p.page}: ${toc.length} sections, ${declared} top-level declarations`);
}
console.log(`build-reference: schema hash ${hash}; ${changed ? `${changed} page(s) updated` : "up to date"}`);
