#!/usr/bin/env node
// Writes the docs sidebar (desktop column and mobile menu) and the previous/next pager into every
// docs page from site/data/docs.json, so a page is added to the navigation in one place.
//
//   node site/scripts/sync-docs-nav.mjs           rewrite the pages
//   node site/scripts/sync-docs-nav.mjs --check   write nothing; exit 1 if a page is out of date
import { join, posix } from "node:path";
import { SITE, read, writeIfChanged, esc } from "./lib.mjs";

const check = process.argv.includes("--check");
const docs = JSON.parse(read(join(SITE, "data", "docs.json")));
const flat = [docs.overview, ...docs.groups.flatMap((g) => g.pages)].filter((p) => !p.external);
/** Pages outside docs/ that wear the sidebar without being in it. */
const also = (docs.also ?? []).map((file) => ({ file }));

/** Site-relative path of a docs.json `file` (relative to docs/): "../reference/swift.html" is "reference/swift.html". */
const sitePath = (file) => posix.join("docs", file);
/** The href from the page at site path `from` to the docs.json `file`. */
const href = (from, file) => {
  const to = sitePath(file), dir = posix.dirname(from);
  if (to === posix.join(dir, "index.html") && dir === "docs") return "./";
  return posix.relative(dir, to);
};

const nav = (from, current) => `<nav aria-label="Docs">${docs.groups.map((g) => `<div class="side-group"><p class="side-title">${esc(g.title)}</p><ul>${g.pages.map((p) => `<li><a href="${p.file === current ? posix.basename(p.file) : href(from, p.file)}"${p.file === current ? ' aria-current="page"' : ""}>${esc(p.title)}</a></li>`).join("")}</ul></div>`).join("")}</nav>`;

function pager(from, current) {
  const i = flat.findIndex((p) => p.file === current);
  const link = (p, cls, label) => `<a class="${cls}" href="${p.file === "index.html" ? "./" : href(from, p.file)}"><span>${label}</span><b>${esc(p.pager || p.title)}</b></a>`;
  return `<nav class="pager" aria-label="Previous and next">${i > 0 ? "\n    " + link(flat[i - 1], "prev", "Previous") : ""}${i < flat.length - 1 ? "\n    " + link(flat[i + 1], "next", "Next") : ""}\n  </nav>`;
}

const stale = [];
for (const p of [...flat, ...also]) {
  const from = sitePath(p.file);
  const file = join(SITE, from);
  let html;
  try { html = read(file); } catch { continue; } // a page that is listed before it exists
  const before = html;
  const mine = also.includes(p) ? null : p.file; // an `also` page marks nothing as current
  html = html.replace(/(<details class="mnav"><summary>Docs menu<\/summary>)<nav[\s\S]*?<\/nav>(<\/details>)/, (m, a, b) => a + nav(from, mine) + b);
  html = html.replace(/(<aside class="sidebar">)<nav[\s\S]*?<\/nav>(<\/aside>)/, (m, a, b) => a + nav(from, mine) + b);
  if (mine) html = html.replace(/<nav class="pager"[\s\S]*?<\/nav>/, pager(from, p.file));
  if (html !== before) { stale.push(from); if (!check) writeIfChanged(file, html); }
}
console.log(stale.length ? `sync-docs-nav: ${check ? "out of date" : "updated"}: ${stale.join(", ")}` : "sync-docs-nav: every docs page is up to date");
process.exit(check && stale.length ? 1 : 0);
