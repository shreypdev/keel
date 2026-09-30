#!/usr/bin/env node
// Builds the blog from the posts: reads every site/blog/<slug>/index.html (title, description, date,
// eyebrow, reading time from the word count) and writes
//   * the cards between <!-- blog:start --> and <!-- blog:end --> in site/blog/index.html,
//   * the Blog JSON-LD between <!-- blog-ld:start --> and <!-- blog-ld:end -->,
//   * site/feed.xml (RSS 2.0),
//   * site/sitemap.xml (the whole site, with lastmod; the blog entries come from the posts).
// With no posts it renders the single "First posts are coming" card, which disappears by itself.
// The "N min read" of each post's .post-meta line is kept equal to the computed value.
//
//   node site/scripts/build-blog-index.mjs
import { readdirSync, existsSync } from "node:fs";
import { join } from "node:path";
import { SITE, ORIGIN, read, writeIfChanged, replaceRegion, esc, titleOf, metaOf, innerOf, textOf, decode, modifiedOf } from "./lib.mjs";
import { writeSitemap } from "./sitemap.mjs";

const WPM = 220;
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const human = (iso) => { const [y, m, d] = iso.split("-").map(Number); return `${d} ${MONTHS[m - 1]} ${y}`; };
const rfc822 = (iso) => new Date(iso + "T00:00:00Z").toUTCString();

const blogDir = join(SITE, "blog");
const slugs = existsSync(blogDir) ? readdirSync(blogDir, { withFileTypes: true }).filter((d) => d.isDirectory() && existsSync(join(blogDir, d.name, "index.html"))).map((d) => d.name) : [];

const posts = slugs.map((slug) => {
  const file = join(blogDir, slug, "index.html");
  let html = read(file);
  const article = innerOf(html, /<article[^>]*>/);
  const prose = article.replace(/<nav class="toc"[\s\S]*?<\/nav>/, "");
  const words = textOf(prose).split(/\s+/).filter(Boolean).length;
  const minutes = Math.max(1, Math.ceil(words / WPM));
  const header = innerOf(html, /<header class="post-header"[^>]*>/);
  const h1 = textOf(innerOf(header, /<h1[^>]*>/)) || titleOf(html).replace(/\s*\|\s*Keel blog$/, "");
  const time = /<time datetime="(\d{4}-\d{2}-\d{2})"/.exec(header);
  if (!time) throw new Error(`blog/${slug}: the post-meta line needs <time datetime="YYYY-MM-DD">`);
  const description = metaOf(html, "description");
  if (!description) throw new Error(`blog/${slug}: missing meta description`);
  // keep the stated reading time equal to the computed one
  const fixed = html.replace(/(<p class="post-meta"[^>]*>[\s\S]*?)(\d+) min read/, (m, pre) => `${pre}${minutes} min read`);
  if (fixed !== html) { writeIfChanged(file, fixed); html = fixed; }
  return { slug, title: h1, description, date: time[1], modified: modifiedOf(html) || time[1], eyebrow: textOf(innerOf(header, /<p class="eyebrow"[^>]*>/)) || "Post", minutes, url: `${ORIGIN}blog/${slug}/` };
}).sort((a, b) => (a.date < b.date ? 1 : a.date > b.date ? -1 : a.title.localeCompare(b.title)));

// ---- index cards
const cards = posts.length
  ? posts.map((p) => [
      '<article class="post-card reveal">',
      `<a href="${p.slug}/"><p class="eyebrow">${esc(p.eyebrow)}</p><h2>${esc(p.title)}</h2><p>${esc(p.description)}</p><p class="post-meta"><time datetime="${p.date}">${human(p.date)}</time> · ${p.minutes} min read</p></a>`,
      "</article>",
    ].join("")).join("\n")
  : '<article class="post-card empty"><p class="eyebrow">Blog</p><h2>First posts are coming</h2><p>Comparisons with Kotlin Multiplatform, UniFFI and Crux, and the reasoning behind keeping your UI native. <a class="rss-link" href="../feed.xml">Subscribe by RSS</a> to get them when they land.</p></article>';

const landing = read(join(SITE, "index.html"));
const newest = posts.reduce((d, p) => (p.modified > d ? p.modified : d), "");
const blogDate = newest || modifiedOf(landing);
const ld = { "@context": "https://schema.org", "@type": "Blog", name: "Keel blog", description: "Comparisons, architecture and design notes from the Keel team.", url: `${ORIGIN}blog/`, inLanguage: "en", dateModified: blogDate, publisher: { "@id": `${ORIGIN}#org` }, blogPost: posts.map((p) => ({ "@type": "BlogPosting", headline: p.title, url: p.url, datePublished: p.date, description: p.description })) };

const indexFile = join(blogDir, "index.html");
let index = read(indexFile);
index = replaceRegion(index, "blog", `<div class="post-grid">\n${cards}\n</div>`, "    ");
index = replaceRegion(index, "blog-ld", `<script type="application/ld+json">\n${JSON.stringify(ld)}\n</script>`, "");
const a = writeIfChanged(indexFile, index);

// ---- RSS 2.0
const items = posts.map((p) => `    <item>\n      <title>${esc(p.title)}</title>\n      <link>${p.url}</link>\n      <guid isPermaLink="true">${p.url}</guid>\n      <pubDate>${rfc822(p.date)}</pubDate>\n      <category>${esc(p.eyebrow)}</category>\n      <description>${esc(p.description)}</description>\n    </item>`);
const rss = ['<?xml version="1.0" encoding="UTF-8"?>', '<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom">', "  <channel>", "    <title>Keel blog</title>", `    <link>${ORIGIN}blog/</link>`, "    <description>Comparisons, architecture and design notes from the Keel team.</description>", "    <language>en</language>", `    <atom:link href="${ORIGIN}feed.xml" rel="self" type="application/rss+xml"/>`, ...(posts.length ? [`    <lastBuildDate>${rfc822(posts[0].date)}</lastBuildDate>`] : []), ...items, "  </channel>", "</rss>", ""].join("\n");
const b = writeIfChanged(join(SITE, "feed.xml"), rss);

const c = writeSitemap(newest);
console.log(`build-blog-index: ${posts.length} post(s); index ${a ? "updated" : "up to date"}, feed ${b ? "updated" : "up to date"}, sitemap ${c ? "updated" : "up to date"}`);
