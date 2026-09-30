#!/usr/bin/env node
// Keeps the shared chrome identical on every page. site/index.html is the source of truth for:
//   * the <header class="site-header"> and <footer class="site-footer"> blocks,
//   * the head boilerplate between <!-- chrome:head --> and <!-- /chrome:head --> (theme-color, icon,
//     RSS link, the theme bootstrap script, the Google Fonts links),
//   * the skip link, and id="main" on <main>.
// Relative href/src values are rewritten for each page's depth (blog posts live at depth 2) and the
// link to the section the page belongs to gets aria-current="page".
//
//   node site/scripts/sync-chrome.mjs           rewrite every page
//   node site/scripts/sync-chrome.mjs --check   write nothing; exit 1 if any page is out of sync
import { posix } from "node:path";
import { SITE, read, writeIfChanged, htmlFiles, rel } from "./lib.mjs";
import { join } from "node:path";

const SKIP = new Set(["404.html", "og/index.html"]); // self-contained pages
const check = process.argv.includes("--check");

const home = read(join(SITE, "index.html"));
const grab = (re) => { const m = re.exec(home); if (!m) throw new Error(`index.html lacks ${re}`); return m[0]; };
const HEADER = grab(/<header class="site-header">[\s\S]*?<\/header>/);
const FOOTER = grab(/<footer class="site-footer">[\s\S]*?<\/footer>/);
const HEAD = grab(/<!-- chrome:head -->[\s\S]*?<!-- \/chrome:head -->/);
const SKIPLINK = '<a class="skip" href="#main">Skip to content</a>';

/** Rewrites root-relative href/src values of a fragment for a page at `depth`. */
function rebase(fragment, depth) {
  const up = "../".repeat(depth);
  return fragment.replace(/\b(href|src)="([^"]*)"/g, (m, attr, v) => {
    if (/^(?:[a-z][a-z0-9+.-]*:|\/\/|\/|#)/i.test(v)) return m;
    return `${attr}="${v === "./" ? up || "./" : up + v}"`;
  });
}

/** Marks the nav link of the section `pagePath` belongs to. */
function markCurrent(header, pagePath) {
  header = header.replace(/ aria-current="page"/g, "");
  const dir = posix.dirname(pagePath);
  return header.replace(/<a ([^>]*?)href="([^"#:]*\/)"([^>]*)>/g, (m, pre, href, post) => {
    if (!/class="nav-gh"|class="brand"/.test(pre + post)) {
      const target = posix.normalize(posix.join(dir, href)); // e.g. "docs/"
      if (target !== "./" && pagePath.startsWith(target)) {
        return `<a ${pre}href="${href}"${post} aria-current="page">`;
      }
    }
    return m;
  });
}

/** What `html` becomes once its chrome is synced. */
function sync(html, pagePath) {
  const depth = pagePath.split("/").length - 1;
  // head boilerplate: replace the marked block, or strip the legacy pieces and insert it
  const head = rebase(HEAD, depth);
  if (html.includes("<!-- chrome:head -->")) {
    html = html.replace(/<!-- chrome:head -->[\s\S]*?<!-- \/chrome:head -->/, () => head);
  } else {
    html = html
      .replace(/<meta name="theme-color"[^>]*>\n?/g, "")
      .replace(/<meta name="color-scheme"[^>]*>\n?/g, "")
      .replace(/<link rel="icon"[^>]*>\n?/g, "")
      .replace(/<script>\(function\(d\)\{var t[\s\S]*?<\/script>\n?/g, "")
      .replace(/<link rel="preconnect" href="https:\/\/fonts\.[a-z.]+"[^>]*>\n?/g, "")
      .replace(/<link rel="preload" as="style" href="https:\/\/fonts\.googleapis[^>]*>\n?/g, "")
      .replace(/<noscript><link rel="stylesheet" href="https:\/\/fonts\.googleapis[^>]*><\/noscript>\n?/g, "");
    const at = html.search(/<link rel="stylesheet"/);
    const i = at >= 0 ? at : html.indexOf("</head>");
    html = html.slice(0, i) + head + "\n" + html.slice(i);
  }
  // skip link, header, footer
  const header = markCurrent(rebase(HEADER, depth), pagePath);
  const footer = rebase(FOOTER, depth);
  if (!/<a class="skip"/.test(html)) html = html.replace(/(<body[^>]*>)\n?/, (m) => m.trimEnd() + "\n" + SKIPLINK + "\n");
  if (/<header class="site-header">/.test(html)) html = html.replace(/<header class="site-header">[\s\S]*?<\/header>/, () => header);
  else html = html.replace(SKIPLINK, SKIPLINK + "\n" + header);
  if (/<footer class="site-footer">/.test(html)) html = html.replace(/<footer class="site-footer">[\s\S]*?<\/footer>/, () => footer);
  else html = html.replace(/\n?(<script src=)/, (m, s) => "\n" + footer + "\n" + s);
  // the skip link needs a target
  if (!/id="main"/.test(html)) html = html.replace(/<main(?![^>]*\bid=)/, '<main id="main"');
  return html;
}

const stale = [];
for (const file of htmlFiles()) {
  const path = rel(file);
  if (SKIP.has(path) || path === "index.html") continue;
  const before = read(file), after = sync(before, path);
  if (before === after) continue;
  stale.push(path);
  if (!check) writeIfChanged(file, after);
}
// index.html itself: only its nav state is derived (no aria-current on the home page)
if (stale.length) console.log(`sync-chrome: ${check ? "out of sync" : "updated"}: ${stale.join(", ")}`);
else console.log("sync-chrome: every page is in sync");
process.exit(check && stale.length ? 1 : 0);
