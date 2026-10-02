#!/usr/bin/env node
// Writes site/llms.txt (an index for language-model agents) and site/llms-full.txt (every docs page, the
// API reference pages, the roadmap and every blog post as Markdown, in one fetch).
//
//   node site/scripts/build-llms.mjs
import { readdirSync, existsSync } from "node:fs";
import { join, posix } from "node:path";
import { SITE, ORIGIN, read, writeIfChanged, innerOf, titleOf, metaOf, textOf, decode, modifiedOf } from "./lib.mjs";

const navData = JSON.parse(read(join(SITE, "data", "docs.json")));
const DOCS_ORDER = ["index", ...navData.groups.flatMap((g) => g.pages.filter((p) => !p.external).map((p) => p.file.replace(/\.html$/, "")))];

// ---- a small HTML -> Markdown converter for the subset the site uses
const VOID = new Set(["br", "hr", "img", "meta", "link", "input", "path", "circle", "rect", "line", "stop", "use"]);
function parse(html) {
  const root = { tag: "#root", attrs: {}, kids: [] };
  const stack = [root];
  const re = /<!--[\s\S]*?-->|<(\/?)([a-zA-Z][\w-]*)((?:\s+[^\s=>\/]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+))?)*)\s*(\/?)>|([^<]+)/g;
  let m;
  while ((m = re.exec(html))) {
    if (m[5] !== undefined) { stack.at(-1).kids.push(m[5]); continue; }
    if (!m[2]) continue;
    const tag = m[2].toLowerCase();
    if (m[1]) { for (let i = stack.length - 1; i > 0; i--) if (stack[i].tag === tag) { stack.length = i; break; } continue; }
    const attrs = {};
    for (const a of m[3].matchAll(/([^\s=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g)) attrs[a[1]] = decode(a[2] ?? a[3] ?? a[4] ?? "");
    const node = { tag, attrs, kids: [] };
    stack.at(-1).kids.push(node);
    if (!VOID.has(tag) && !m[4]) stack.push(node);
  }
  return root;
}
const plain = (n) => (typeof n === "string" ? decode(n) : n.kids.map(plain).join(""));
const abs = (href, page) => (/^[a-z][a-z0-9+.-]*:/i.test(href) ? href : href.startsWith("#") ? ORIGIN + page + href : new URL(href, ORIGIN + page).href);

function inline(n, page) {
  if (typeof n === "string") return decode(n).replace(/\s+/g, " ");
  const k = () => n.kids.map((c) => inline(c, page)).join("");
  switch (n.tag) {
    case "code": return "`" + plain(n).replace(/\s+/g, " ").trim() + "`";
    case "strong": case "b": return "**" + k().trim() + "**";
    case "em": case "i": return "*" + k().trim() + "*";
    case "a": return n.attrs.class === "anchor" ? "" : n.attrs.href ? `[${k().trim()}](${abs(n.attrs.href, page)})` : k();
    case "br": return "\n";
    case "svg": case "script": case "style": case "button": return "";
    default: return k();
  }
}
function block(n, page, depth = 0) {
  if (typeof n === "string") { const t = decode(n).replace(/\s+/g, " ").trim(); return t ? t + "\n\n" : ""; }
  if (/\b(edit|pager|toc|crumb|cards|skip)\b/.test(n.attrs.class || "")) return "";
  const kids = () => n.kids.map((c) => block(c, page, depth)).join("");
  const inl = () => n.kids.map((c) => inline(c, page)).join("").replace(/[ \t]+/g, " ").trim();
  switch (n.tag) {
    case "h1": case "h2": case "h3": case "h4": return `${"#".repeat(+n.tag[1])} ${inl()}\n\n`;
    case "p": { const t = inl(); return t ? t + "\n\n" : ""; }
    case "pre": { const c = n.kids.find((x) => typeof x !== "string" && x.tag === "code"); const lang = (c && (c.attrs["data-lang"] || /language-(\w+)/.exec(c.attrs.class || "")?.[1])) || ""; return "```" + lang + "\n" + plain(c || n).replace(/\n$/, "") + "\n```\n\n"; }
    case "ul": case "ol": return n.kids.filter((c) => typeof c !== "string" && c.tag === "li").map((li, i) => {
      const nested = li.kids.filter((c) => typeof c !== "string" && (c.tag === "ul" || c.tag === "ol"));
      const text = li.kids.filter((c) => !nested.includes(c)).map((c) => inline(c, page)).join("").replace(/\s+/g, " ").trim();
      const sub = nested.map((c) => block(c, page, depth + 1).trimEnd()).join("\n");
      return `${"  ".repeat(depth)}${n.tag === "ol" ? i + 1 + "." : "-"} ${text}${sub ? "\n" + sub : ""}`;
    }).join("\n") + (depth ? "" : "\n\n") + (depth ? "" : "");
    case "table": {
      const rows = [];
      (function walk(x) { if (typeof x === "string") return; if (x.tag === "tr") rows.push(x.kids.filter((c) => typeof c !== "string").map((c) => inline(c, page).replace(/\s+/g, " ").replace(/\|/g, "\\|").trim())); else x.kids.forEach(walk); })(n);
      if (!rows.length) return "";
      const w = Math.max(...rows.map((r) => r.length));
      const pad = (r) => "| " + Array.from({ length: w }, (_, i) => r[i] ?? "").join(" | ") + " |";
      return [pad(rows[0]), pad(Array(w).fill("---")), ...rows.slice(1).map(pad)].join("\n") + "\n\n";
    }
    case "figure": { const cap = n.kids.find((c) => typeof c !== "string" && c.tag === "figcaption"); return cap ? `*Figure: ${inline(cap, page).trim()}*\n\n` : ""; }
    case "aside": case "blockquote": return kids().trim().split("\n").map((l) => (l ? "> " + l : ">")).join("\n") + "\n\n";
    case "nav": case "script": case "style": case "svg": case "button": case "details": return "";
    case "hr": return "---\n\n";
    default: return kids();
  }
}
const toMarkdown = (html, page) => block(parse(html), page).replace(/\n{3,}/g, "\n\n").trim() + "\n";

// ---- collect pages
const pages = [];
const docs = readdirSync(join(SITE, "docs")).filter((f) => f.endsWith(".html")).map((f) => f.slice(0, -5));
// the pages in subdirectories of docs/ (the cookbook), in the order docs.json lists them
docs.push(...navData.groups.flatMap((g) => g.pages).filter((p) => !p.external && p.file.includes("/") && !p.file.startsWith("..")).map((p) => p.file.replace(/\.html$/, "")));
docs.sort((a, b) => ((DOCS_ORDER.indexOf(a) + 1 || 99) - (DOCS_ORDER.indexOf(b) + 1 || 99)) || a.localeCompare(b));
for (const d of docs) pages.push({ group: "Docs", path: d === "index" ? "docs/" : d.endsWith("/index") ? `docs/${d.slice(0, -5)}` : `docs/${d}.html`, file: `docs/${d}.html` });
for (const f of navData.also ?? []) { // the generated API reference pages, in the order docs.json lists them
  const file = posix.normalize(posix.join("docs", f));
  if (existsSync(join(SITE, file))) pages.push({ group: "Reference", path: file, file });
}
if (existsSync(join(SITE, "roadmap/index.html"))) pages.push({ group: "Roadmap", path: "roadmap/", file: "roadmap/index.html" });
const blogDir = join(SITE, "blog");
const posts = existsSync(blogDir) ? readdirSync(blogDir, { withFileTypes: true }).filter((d) => d.isDirectory() && existsSync(join(blogDir, d.name, "index.html"))).map((d) => d.name) : [];
const dated = posts.map((slug) => ({ slug, html: read(join(blogDir, slug, "index.html")) })).map((p) => ({ ...p, date: modifiedOf(p.html) })).sort((a, b) => (a.date < b.date ? 1 : -1));
for (const p of dated) pages.push({ group: "Blog", path: `blog/${p.slug}/`, file: `blog/${p.slug}/index.html` });

for (const p of pages) {
  p.html = read(join(SITE, p.file));
  p.title = titleOf(p.html);
  p.desc = metaOf(p.html, "description");
  p.url = ORIGIN + p.path;
}

// ---- llms.txt
const groups = ["Docs", "Reference", "Roadmap", "Blog"];
const lines = [
  "# Undra",
  "",
  "> Undra is a Rust framework that owns everything under the pixels of native iOS, Android and web apps: domain logic, reactive state, the data layer, persistence and the dev loop. It generates idiomatic Swift, Kotlin and TypeScript from one Rust core, and the UI stays SwiftUI, Jetpack Compose and React. Not to be confused with undra.sh (Kubernetes) or undra.so.",
  "",
  "Undra is open source (MIT OR Apache-2.0) and ships an `undra` CLI (`undra init`, `undra dev`, `undra build`, `undra bindgen`, `undra doctor`). Reads never cross the language boundary: each platform holds a mirror of the state, updated by one binary change-set per transaction.",
  "",
];
for (const g of groups) {
  const list = pages.filter((p) => p.group === g);
  if (!list.length) continue;
  const rust = g === "Reference" ? [`- [Rust API reference](${ORIGIN}reference/rust/undra/index.html): rustdoc for the undra crate and the runtime, signals, query, ports, wire and meta crates it re-exports`] : [];
  lines.push(`## ${g}`, "", ...list.map((p) => `- [${p.title}](${p.url}): ${p.desc}`), ...rust, "");
}
lines.push("## Source and numbers", "", "- [GitHub repository](https://github.com/shreypdev/undra): source, issues and releases", "- [Specification](https://github.com/shreypdev/undra/blob/main/docs/SPEC.md): the binding implementation spec (wire format, ABI, schema)", "- [Benchmark results](https://github.com/shreypdev/undra/blob/main/bench/RESULTS.md): host-measured numbers and budgets", "");
lines.push("## Optional", "", `- [All documentation in one file](${ORIGIN}llms-full.txt): every docs page, the roadmap and every post as Markdown`, `- [RSS feed](${ORIGIN}feed.xml)`, "");
const a = writeIfChanged(join(SITE, "llms.txt"), lines.join("\n"));

// ---- llms-full.txt
const full = ["# Undra: full documentation", "", `> Everything on ${ORIGIN} that explains Undra, as Markdown: the docs, the roadmap and the blog. Index: ${ORIGIN}llms.txt`, ""];
for (const p of pages) {
  const main = innerOf(p.html, /<main[^>]*>/) || innerOf(p.html, /<article[^>]*>/);
  full.push("---", "", `Source: ${p.url}`, "", toMarkdown(main, p.path));
}
const b = writeIfChanged(join(SITE, "llms-full.txt"), full.join("\n") + "\n");
console.log(`build-llms: ${pages.length} pages; llms.txt ${a ? "updated" : "up to date"}, llms-full.txt ${b ? "updated" : "up to date"}`);
