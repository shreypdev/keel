#!/usr/bin/env node
// Builds site/search-index.json for the Cmd/Ctrl-K search (site/assets/search.js): one entry per page
// and one per heading of the docs, the roadmap and every blog post, with the text under it.
// Entries: { u: url relative to the site root, p: page title, h: heading ("" for the page itself), x: text }.
//
//   node site/scripts/build-search-index.mjs
import { readdirSync, existsSync } from "node:fs";
import { join } from "node:path";
import { SITE, read, writeIfChanged, innerOf, textOf, titleOf, decode, rel } from "./lib.mjs";

const DOCS_ORDER = ["index", "getting-started", "concepts", "queries", "ports", "cli", "architecture", "api-swift", "api-kotlin", "api-typescript"];
const CAP = 1200;
const clip = (t) => (t.length <= CAP ? t : t.slice(0, CAP).replace(/\s+\S*$/, "") + " …");

function pages() {
  const docs = readdirSync(join(SITE, "docs")).filter((f) => f.endsWith(".html")).map((f) => f.slice(0, -5));
  docs.sort((a, b) => ((DOCS_ORDER.indexOf(a) + 1 || 99) - (DOCS_ORDER.indexOf(b) + 1 || 99)) || a.localeCompare(b));
  const out = docs.map((d) => `docs/${d}.html`);
  if (existsSync(join(SITE, "roadmap/index.html"))) out.push("roadmap/index.html");
  const blog = join(SITE, "blog");
  if (existsSync(blog)) for (const d of readdirSync(blog, { withFileTypes: true }).filter((x) => x.isDirectory()).map((x) => x.name).sort()) out.push(`blog/${d}/index.html`);
  return out;
}

const urlOf = (path) => (path.endsWith("/index.html") ? path.slice(0, -"index.html".length) : path);

function entries(path) {
  const html = read(join(SITE, path));
  let main = innerOf(html, /<main[^>]*>/) || innerOf(html, /<article[^>]*>/);
  main = main.replace(/<nav class="(?:pager|toc)"[\s\S]*?<\/nav>/g, "").replace(/<p class="edit">[\s\S]*?<\/p>/g, "");
  const h1 = textOf(innerOf(main, /<h1[^>]*>/)) || titleOf(html);
  const page = h1;
  const out = [];
  // sections start at each h2/h3; blog headings sit inside <section id>, docs headings carry the id
  const re = /(?:<section[^>]*\sid="([^"]+)"[^>]*>\s*)?<h([123])([^>]*)>([\s\S]*?)<\/h\2>/g;
  const marks = [];
  let m;
  while ((m = re.exec(main))) {
    const own = /id="([^"]+)"/.exec(m[3]);
    marks.push({ level: +m[2], id: own ? own[1] : m[1] || "", title: textOf(m[4]), start: m.index, end: m.index + m[0].length });
  }
  const intro = marks.length ? main.slice(marks[0].end, marks.length > 1 ? marks[1].start : main.length) : main;
  out.push({ u: urlOf(path), p: page, h: "", x: clip(textOf(intro)) });
  marks.forEach((k, i) => {
    if (k.level === 1 || !k.id) return;
    const body = main.slice(k.end, i + 1 < marks.length ? marks[i + 1].start : main.length);
    out.push({ u: `${urlOf(path)}#${k.id}`, p: page, h: k.title, x: clip(textOf(body)) });
  });
  return out;
}

const all = pages().flatMap(entries);
const json = "[\n" + all.map((e) => JSON.stringify(e)).join(",\n") + "\n]\n";
console.log(`build-search-index: ${all.length} entries, ${(json.length / 1024).toFixed(0)} KB; ` + (writeIfChanged(join(SITE, "search-index.json"), json) ? "updated" : "up to date"));
