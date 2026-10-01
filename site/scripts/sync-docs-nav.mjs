#!/usr/bin/env node
// Writes the docs sidebar (desktop column and mobile menu) and the previous/next pager into every
// docs page from site/data/docs.json, so a page is added to the navigation in one place.
//
//   node site/scripts/sync-docs-nav.mjs           rewrite the pages
//   node site/scripts/sync-docs-nav.mjs --check   write nothing; exit 1 if a page is out of date
import { join } from "node:path";
import { SITE, read, writeIfChanged, esc } from "./lib.mjs";

const check = process.argv.includes("--check");
const docs = JSON.parse(read(join(SITE, "data", "docs.json")));
const flat = [docs.overview, ...docs.groups.flatMap((g) => g.pages)];

const nav = (current) => `<nav aria-label="Docs">${docs.groups.map((g) => `<div class="side-group"><p class="side-title">${esc(g.title)}</p><ul>${g.pages.map((p) => `<li><a href="${p.file}"${p.file === current ? ' aria-current="page"' : ""}>${esc(p.title)}</a></li>`).join("")}</ul></div>`).join("")}</nav>`;

function pager(current) {
  const i = flat.findIndex((p) => p.file === current);
  const link = (p, cls, label) => `<a class="${cls}" href="${p.file === "index.html" ? "./" : p.file}"><span>${label}</span><b>${esc(p.pager || p.title)}</b></a>`;
  return `<nav class="pager" aria-label="Previous and next">${i > 0 ? "\n    " + link(flat[i - 1], "prev", "Previous") : ""}${i < flat.length - 1 ? "\n    " + link(flat[i + 1], "next", "Next") : ""}\n  </nav>`;
}

const stale = [];
for (const p of flat) {
  const file = join(SITE, "docs", p.file);
  let html;
  try { html = read(file); } catch { continue; } // a page that is listed before it exists
  const before = html;
  html = html.replace(/(<details class="mnav"><summary>Docs menu<\/summary>)<nav[\s\S]*?<\/nav>(<\/details>)/, (m, a, b) => a + nav(p.file) + b);
  html = html.replace(/(<aside class="sidebar">)<nav[\s\S]*?<\/nav>(<\/aside>)/, (m, a, b) => a + nav(p.file) + b);
  html = html.replace(/<nav class="pager"[\s\S]*?<\/nav>/, pager(p.file));
  if (html !== before) { stale.push(`docs/${p.file}`); if (!check) writeIfChanged(file, html); }
}
console.log(stale.length ? `sync-docs-nav: ${check ? "out of date" : "updated"}: ${stale.join(", ")}` : "sync-docs-nav: every docs page is up to date");
process.exit(check && stale.length ? 1 : 0);
