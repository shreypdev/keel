#!/usr/bin/env node
// Checks the built site. Exits 1 and lists every failure when:
//   * an internal link, asset or fragment does not resolve to a file or an id,
//   * a page lacks a title (<= 60 chars), description (<= 155), canonical (equal to its URL),
//     og:title/og:description/og:url/og:image, twitter:card summary_large_image, a single <h1>,
//     or exactly one valid JSON-LD block,
//   * two pages share a title or a description,
//   * the shared header/footer/head boilerplate differs from site/index.html (sync-chrome --check).
//
//   node site/scripts/check-links.mjs                 checks site/
//   node site/scripts/check-links.mjs --root _site    checks the staged site, playground included
import { existsSync, readFileSync } from "node:fs";
import { join, resolve, relative, sep, posix } from "node:path";
import { spawnSync } from "node:child_process";
import { SITE, ORIGIN, htmlFiles, read, titleOf, metaOf, canonicalOf, jsonLd, stripElements, decode } from "./lib.mjs";

/** The landing page is for scanning, not reading: at most this many words of visible prose. */
export const LANDING_WORD_BUDGET = 350;

/** Words of visible prose on the landing page: everything except the site header and footer, code blocks,
 *  the install blocks, the numbers grid, diagrams, scripts, and what a closed <details> hides. */
export function landingWords(html) {
  let h = html.slice(html.indexOf("<body"));
  for (const open of [/<header class="site-header"[^>]*>/, /<footer class="site-footer"[^>]*>/, /<script\b[^>]*>/, /<style\b[^>]*>/, /<svg\b[^>]*>/, /<noscript\b[^>]*>/, /<pre\b[^>]*>/, /<div class="stats"[^>]*>/, /<div class="install"[^>]*>/, /<a class="skip"[^>]*>/, /<ul class="seg-key"[^>]*>/, /<div class="gridcells"[^>]*>/, /<button\b[^>]*\bhidden\b[^>]*>/]) h = stripElements(h, open);
  h = h.replace(/<details\b[^>]*>\s*(<summary\b[^>]*>[\s\S]*?<\/summary>)[\s\S]*?<\/details>/g, "$1");
  return decode(h.replace(/<[^>]+>/g, " ")).split(/\s+/).filter((w) => /[A-Za-z0-9]/.test(w));
}

const args = process.argv.slice(2);
if (args.includes("--words")) {
  const words = landingWords(read(join(SITE, "index.html")));
  console.log(`landing prose: ${words.length} words (budget ${LANDING_WORD_BUDGET})`);
  if (args.includes("--show")) console.log(words.join(" "));
  process.exit(words.length > LANDING_WORD_BUDGET ? 1 : 0);
}
const rootArg = args.indexOf("--root");
const ROOT = rootArg >= 0 ? resolve(args[rootArg + 1]) : SITE;
const failures = [], notes = [];
const fail = (page, msg) => failures.push(`${page}: ${msg}`);
const pending = new Set(existsSync(join(SITE, "data/pending.json")) ? JSON.parse(read(join(SITE, "data/pending.json"))).pages : []);

// What CI builds into _site and the repository does not hold: present only when checking a staged site.
// The playground is an app build and reference/rust/ is rustdoc's output (its own pages and links are
// rustdoc's to get right, under -D warnings); a page of the site that links into either is checked
// against the staged tree and skipped in the source tree.
const hasPlayground = existsSync(join(ROOT, "playground"));
const hasRustdoc = existsSync(join(ROOT, "reference/rust"));
const BUILT = (p) => (!hasPlayground && p.startsWith("playground/")) || (!hasRustdoc && p.startsWith("reference/rust/"));
const RUSTDOC = "reference/rust/";
const EXEMPT = new Set(["404.html", "og/index.html"]); // self-contained pages with no SEO contract

const pathOf = (f) => relative(ROOT, f).split(sep).join("/");
const files = htmlFiles(ROOT).filter((f) => !pathOf(f).startsWith(RUSTDOC));
const ids = new Map(); // page path -> Set of ids
const idsOf = (html) => new Set([...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]));
for (const f of files) ids.set(pathOf(f), idsOf(read(f)));

/** The site-relative file a URL on `page` points at, "" for external/ignored links. */
function target(page, url) {
  if (/^(?:mailto:|tel:|data:|javascript:)/i.test(url)) return null;
  if (/^https?:\/\//i.test(url)) return url.startsWith(ORIGIN) ? url.slice(ORIGIN.length) : null;
  if (url.startsWith("//")) return null;
  if (url.startsWith("/undra/")) return url.slice("/undra/".length); // root-absolute under the Pages base path: the 404 page and the playground build
  if (url.startsWith("/")) return url.slice(1);
  return posix.join(posix.dirname(page), url);
}

const titles = new Map(), descs = new Map();
for (const f of files) {
  const page = pathOf(f);
  const html = read(f);
  if (page === "og/index.html") continue;

  // ---- links and assets
  for (const m of html.matchAll(/<(a|link|script|img|iframe|source)\b[^>]*?\s(href|src|data-src)="([^"]*)"/g)) {
    const raw = m[3].replace(/&amp;/g, "&");
    if (!raw || raw.startsWith("#") && raw.length === 1) continue;
    const [noHash, frag] = raw.split("#");
    const [bare] = noHash.split("?");
    let t = raw.startsWith("#") ? page : target(page, bare);
    if (t === null) continue;
    if (EXEMPT.has(page) && raw.startsWith("/") && !raw.startsWith("/undra/")) continue; // the 404 page's fallback for a site served at the root
    if (t === "" || t.endsWith("/")) t += "index.html";
    if (BUILT(t)) continue;
    const exists = existsSync(join(ROOT, t));
    if (!exists) {
      if (pending.has(bare.startsWith("/") ? bare : posix.join(posix.dirname(page), bare)) || pending.has(t.replace(/index\.html$/, ""))) { notes.push(`${page}: pending link "${raw}" (page not merged yet)`); continue; }
      fail(page, `broken ${m[2]} "${raw}" (${t} does not exist)`); continue;
    }
    if (pending.has(t.replace(/index\.html$/, ""))) notes.push(`data/pending.json lists ${t}, which now exists: remove it from the list`);
    if (frag && t.endsWith(".html")) {
      const set = ids.get(t) ?? idsOf(read(join(ROOT, t)));
      if (!set.has(decodeURIComponent(frag))) fail(page, `broken fragment "${raw}" (no id="${frag}" in ${t})`);
    }
  }
  if (EXEMPT.has(page) || page.startsWith("playground/")) continue; // the playground is an app build, not a content page

  // ---- per-page SEO contract
  const expectedUrl = ORIGIN + (page === "index.html" ? "" : page.endsWith("/index.html") ? page.slice(0, -"index.html".length) : page);
  const title = titleOf(html), desc = metaOf(html, "description");
  if (!title) fail(page, "no <title>"); else if (title.length > 60) fail(page, `title is ${title.length} chars (max 60): ${title}`);
  if (!desc) fail(page, "no meta description"); else if (desc.length > 155) fail(page, `description is ${desc.length} chars (max 155)`);
  if (title && titles.has(title)) fail(page, `title duplicates ${titles.get(title)}`); else titles.set(title, page);
  if (desc && descs.has(desc)) fail(page, `description duplicates ${descs.get(desc)}`); else descs.set(desc, page);
  const canon = canonicalOf(html);
  if (!canon) fail(page, "no canonical link"); else if (canon !== expectedUrl) fail(page, `canonical is ${canon}, expected ${expectedUrl}`);
  for (const k of ["og:title", "og:description", "og:url", "og:image"]) if (!metaOf(html, k)) fail(page, `no ${k}`);
  if (metaOf(html, "og:image") !== ORIGIN + "assets/og.png") fail(page, `og:image is not ${ORIGIN}assets/og.png`);
  if (metaOf(html, "twitter:card") !== "summary_large_image") fail(page, "twitter:card is not summary_large_image");
  if (!metaOf(html, "twitter:image")) fail(page, "no twitter:image");
  const h1 = (html.match(/<h1[\s>]/g) || []).length;
  if (h1 !== 1) fail(page, `${h1} <h1> elements (need exactly 1)`);
  if (!/<html lang="en"/.test(html)) fail(page, "no lang on <html>");
  const blocks = (html.match(/<script type="application\/ld\+json">/g) || []).length;
  if (blocks !== 1) fail(page, `${blocks} JSON-LD blocks (need exactly 1)`);
  else { try { jsonLd(html); } catch (e) { fail(page, `invalid JSON-LD: ${e.message}`); } }
  if (page === "index.html") { const n = landingWords(html).length; if (n > LANDING_WORD_BUDGET) fail(page, `the landing page has ${n} words of visible prose (budget ${LANDING_WORD_BUDGET}); cut text, do not raise the budget`); }
  if (/italic/i.test(html.replace(/<script[\s\S]*?<\/script>/g, ""))) fail(page, "mentions italic in markup");
  for (const m of html.matchAll(/<img\b[^>]*>/g)) if (!/\balt="/.test(m[0])) fail(page, "an <img> has no alt");
}
if (!existsSync(join(ROOT, "assets/og.png"))) fail("assets/og.png", "missing (run site/scripts/render-og.sh)");

// ---- shared chrome must equal site/index.html's (only meaningful for the source tree)
if (ROOT === SITE) {
  const r = spawnSync(process.execPath, [join(SITE, "scripts/sync-chrome.mjs"), "--check"], { encoding: "utf8" });
  if (r.status !== 0) fail("chrome", (r.stdout + r.stderr).trim() + " (run: node site/scripts/sync-chrome.mjs)");
}

if (failures.length) {
  console.error(`check-links: ${failures.length} problem(s) in ${files.length} pages\n` + failures.map((f) => "  " + f).join("\n"));
  process.exit(1);
}
for (const n of [...new Set(notes)]) console.log("  note: " + n);
console.log(`check-links: ${files.length} pages OK (${ROOT === SITE ? "site/" : relative(process.cwd(), ROOT) + "/"})`);
