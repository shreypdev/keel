// sitemap.xml for the whole site, written by build-blog-index.mjs so the blog entries and the rest stay together.
import { join } from "node:path";
import { SITE, read, writeIfChanged, htmlFiles, rel, pageUrl, modifiedOf, esc } from "./lib.mjs";

const SKIP = new Set(["404.html", "og/index.html"]);

/** Every indexable page as { url, lastmod }, landing first, then by path. */
export function sitemapEntries(blogDate) {
  const out = [];
  for (const file of htmlFiles()) {
    const path = rel(file);
    if (SKIP.has(path)) continue;
    const html = read(file);
    if (/<meta name="robots" content="[^"]*noindex/i.test(html)) continue;
    let lastmod = modifiedOf(html);
    if (path === "blog/index.html" && blogDate) lastmod = blogDate;
    out.push({ path, url: pageUrl(file), lastmod });
  }
  const order = (p) => (p === "index.html" ? "0" : p === "docs/index.html" ? "1" : p.startsWith("docs/") ? "2" + p : "3" + p);
  return out.sort((a, b) => (order(a.path) < order(b.path) ? -1 : 1));
}

/** Writes site/sitemap.xml. Throws when a page declares no modification date. */
export function writeSitemap(blogDate) {
  const entries = sitemapEntries(blogDate);
  for (const e of entries) if (!e.lastmod) throw new Error(`${e.path}: no dateModified/datePublished in its JSON-LD (the sitemap needs a lastmod)`);
  const xml = ['<?xml version="1.0" encoding="UTF-8"?>', '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">', ...entries.map((e) => `  <url><loc>${esc(e.url)}</loc><lastmod>${e.lastmod}</lastmod></url>`), "</urlset>", ""].join("\n");
  return writeIfChanged(join(SITE, "sitemap.xml"), xml);
}
