// Shared helpers for the site scripts. Node 20+, no dependencies.
import { readFileSync, writeFileSync, readdirSync, lstatSync, existsSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

/** Absolute path of `site/`. */
export const SITE = resolve(dirname(fileURLToPath(import.meta.url)), "..");
/** The canonical origin of the published site, with the trailing slash. */
export const ORIGIN = "https://shreypdev.github.io/undra/";

/** Reads a UTF-8 file. */
export const read = (path) => readFileSync(path, "utf8");

/** Writes `text` only when it differs, so untouched files keep their mtime. Returns true when it wrote. */
export function writeIfChanged(path, text) {
  if (existsSync(path) && read(path) === text) return false;
  writeFileSync(path, text);
  return true;
}

/** Every `.html` file below `dir`, sorted, as absolute paths. `_site`, `node_modules` and dot directories are skipped. */
export function htmlFiles(dir = SITE) {
  const out = [];
  (function walk(d) {
    for (const name of readdirSync(d).sort()) {
      if (name.startsWith(".") || name === "node_modules" || name === "_site") continue;
      const p = join(d, name);
      const st = lstatSync(p);
      if (st.isSymbolicLink()) continue; // the local preview symlinks _site/undra -> .
      if (st.isDirectory()) walk(p);
      else if (name.endsWith(".html")) out.push(p);
    }
  })(dir);
  return out;
}

/** Path of `file` relative to `site/`, with forward slashes. */
export const rel = (file) => relative(SITE, file).split(sep).join("/");

/** The public URL of a page file: `docs/cli.html` -> `.../docs/cli.html`, `blog/x/index.html` -> `.../blog/x/`. */
export function pageUrl(file) {
  const r = rel(file);
  return ORIGIN + (r === "index.html" ? "" : r.endsWith("/index.html") ? r.slice(0, -"index.html".length) : r);
}

/** Replaces what lies between `<!-- name:start -->` and `<!-- name:end -->` (markers kept). Throws if a marker is missing. */
export function replaceRegion(html, name, inner, indent = "    ") {
  const a = `<!-- ${name}:start -->`, b = `<!-- ${name}:end -->`;
  const i = html.indexOf(a), j = html.indexOf(b);
  if (i < 0 || j < 0 || j < i) throw new Error(`marker <!-- ${name}:start/end --> not found`);
  return html.slice(0, i + a.length) + "\n" + inner.replace(/^/gm, indent).replace(/^\s+$/gm, "") + "\n" + indent + html.slice(j);
}

const ENT = { "&amp;": "&", "&lt;": "<", "&gt;": ">", "&quot;": '"', "&#39;": "'", "&apos;": "'", "&nbsp;": " ", "&mdash;": "—", "&ndash;": "–", "&hellip;": "…", "&rarr;": "→" };
/** Decodes the HTML entities the site uses. */
export const decode = (s) => s.replace(/&(?:amp|lt|gt|quot|#39|apos|nbsp|mdash|ndash|hellip|rarr);/g, (m) => ENT[m]).replace(/&#(\d+);/g, (_, n) => String.fromCodePoint(+n)).replace(/&#x([0-9a-f]+);/gi, (_, n) => String.fromCodePoint(parseInt(n, 16)));
/** Escapes text for HTML or XML. */
export const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

/** The content of the first `<title>`, decoded. */
export const titleOf = (html) => decode((/<title>([\s\S]*?)<\/title>/i.exec(html) || [, ""])[1]).trim();
/** The `content` of `<meta name|property="key">`, decoded, or "". */
export function metaOf(html, key) {
  const re = new RegExp(`<meta\\s+(?:name|property)="${key.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}"\\s+content="([^"]*)"`, "i");
  return decode((re.exec(html) || [, ""])[1]).trim();
}
/** The canonical URL, or "". */
export const canonicalOf = (html) => (/<link\s+rel="canonical"\s+href="([^"]*)"/i.exec(html) || [, ""])[1];
/** Every JSON-LD block of a page, as parsed objects (throws on invalid JSON). */
export function jsonLd(html) {
  return [...html.matchAll(/<script type="application\/ld\+json">([\s\S]*?)<\/script>/g)].map((m) => JSON.parse(m[1]));
}
/** The most specific modification date (YYYY-MM-DD) a page declares in its JSON-LD, or "". */
export function modifiedOf(html) {
  const m = /"dateModified"\s*:\s*"(\d{4}-\d{2}-\d{2})/.exec(html) || /"datePublished"\s*:\s*"(\d{4}-\d{2}-\d{2})/.exec(html);
  return m ? m[1] : "";
}

/** The inner HTML of the first element `<tag ...>…</tag>` matched by `open` (a regex for the opening tag), tags balanced. */
export function innerOf(html, open) {
  const m = open.exec(html); if (!m) return "";
  const tag = /^<(\w+)/.exec(m[0])[1], start = m.index + m[0].length;
  const re = new RegExp(`<(/?)${tag}\\b[^>]*>`, "gi"); re.lastIndex = start;
  let depth = 1, x;
  while ((x = re.exec(html))) { depth += x[1] ? -1 : 1; if (!depth) return html.slice(start, x.index); }
  return html.slice(start);
}

/** Plain text of an HTML fragment: tags dropped, entities decoded, whitespace collapsed. Code keeps its text. */
export function textOf(fragment) {
  const drop = fragment.replace(/<script[\s\S]*?<\/script>|<style[\s\S]*?<\/style>|<svg[\s\S]*?<\/svg>/g, " ").replace(/<a class="anchor"[^>]*>.*?<\/a>/g, "");
  const blocks = drop.replace(/<\/?(?:p|div|li|ul|ol|h[1-6]|tr|td|th|table|thead|tbody|pre|br|section|article|nav|header|footer|figure|figcaption|blockquote|aside|dd|dt|dl|details|summary)\b[^>]*>/gi, " ");
  return decode(blocks.replace(/<[^>]+>/g, "")).replace(/\s+/g, " ").trim();
}

/** `html` without every element whose opening tag matches `open` (a regex for the tag), tags balanced. */
export function stripElements(html, open) {
  const re = new RegExp(open.source, open.flags.replace("g", "") + "g");
  let out = "", last = 0, m;
  while ((m = re.exec(html))) {
    if (m.index < last) continue;
    const tag = /^<(\w+)/.exec(m[0])[1];
    const scan = new RegExp(`<(/?)${tag}\\b[^>]*>`, "gi"); scan.lastIndex = m.index + m[0].length;
    let depth = 1, x, end = html.length;
    while ((x = scan.exec(html))) { depth += x[1] ? -1 : 1; if (!depth) { end = x.index + x[0].length; break; } }
    out += html.slice(last, m.index) + " ";
    last = end; re.lastIndex = end;
  }
  return out + html.slice(last);
}
