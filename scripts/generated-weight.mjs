#!/usr/bin/env node
// What the lines of a generated tree are (ADR-062): per platform and per kind of generated
// file, how many lines are code, doc comments, other comments and blank lines.
//
//   node scripts/generated-weight.mjs                      the playground's tree, as a table
//   node scripts/generated-weight.mjs examples/fieldbook/generated
//   node scripts/generated-weight.mjs --json               the same numbers as JSON
//
// The classes are the ones a reader would give: a doc comment is `///` (Swift) or a `/** .. */`
// block (Kotlin, TypeScript, C); any other comment, the header and the `// Swift mode` line
// among them, is "comment"; a line with only whitespace is blank; everything else is code. The
// reader is deliberately simple (it reads the shape bindgen emits, not the languages), so a
// line that carries code and a trailing comment counts as code.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { basename, extname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

/** The three platform trees of a generated directory, in the order the CLI lists them. */
export const PLATFORMS = [
  { dir: "swift", name: "Swift" },
  { dir: "kotlin", name: "Kotlin" },
  { dir: "ts", name: "TypeScript" },
];

/** The kinds of generated source file, by file stem (case-insensitive); the rest is "package". */
const KINDS = ["types", "errors", "objects", "stores", "ports", "queries", "callbacks", "ids", "core", "index"];

/** How a file spells its comments. */
function commentStyle(path) {
  const name = basename(path);
  const ext = extname(path);
  if ([".swift", ".kts"].includes(ext) || name === "module.modulemap" || name === "tsconfig.json") return { line: "//", block: false };
  if ([".kt", ".ts"].includes(ext)) return { line: "//", block: true };
  if ([".c", ".h"].includes(ext)) return { line: null, block: true };
  if (ext === ".pro" || name === ".gitignore" || name === ".gitattributes") return { line: "#", block: false };
  return { line: null, block: false };
}

/**
 * Counts the lines of `text` by class.
 * @param {string} text
 * @param {string} path  only its name and extension are read (they say which comments the file has)
 * @returns {{code: number, doc: number, comment: number, blank: number}}
 */
export function classify(text, path) {
  const style = commentStyle(path);
  const counts = { code: 0, doc: 0, comment: 0, blank: 0 };
  const lines = text.split("\n");
  if (lines.at(-1) === "") lines.pop();
  let inBlock = null; // "doc" or "comment" while inside a /* .. */ block
  for (const raw of lines) {
    const line = raw.trim();
    if (inBlock) {
      counts[inBlock] += 1;
      if (line.includes("*/")) inBlock = null;
      continue;
    }
    if (line === "") counts.blank += 1;
    else if (style.line === "//" && line.startsWith("///")) counts.doc += 1;
    else if (style.block && line.startsWith("/*")) {
      const kind = line.startsWith("/**") && !line.startsWith("/**/") ? "doc" : "comment";
      counts[kind] += 1;
      if (!line.includes("*/", 2)) inBlock = kind;
    } else if (style.line && line.startsWith(style.line)) counts.comment += 1;
    else counts.code += 1;
  }
  return counts;
}

/** The kind a file is counted under: `Types`, `Stores`, .., or `package` for everything else. */
export function kindOf(path) {
  const stem = basename(path, extname(path)).toLowerCase();
  return KINDS.includes(stem) && [".swift", ".kt", ".ts"].includes(extname(path))
    ? stem[0].toUpperCase() + stem.slice(1)
    : "package";
}

function* walk(dir) {
  for (const entry of readdirSync(dir).sort()) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) yield* walk(path);
    else yield path;
  }
}

const ZERO = () => ({ files: 0, lines: 0, code: 0, doc: 0, comment: 0, blank: 0 });

function add(into, counts) {
  into.files += 1;
  into.code += counts.code;
  into.doc += counts.doc;
  into.comment += counts.comment;
  into.blank += counts.blank;
  into.lines += counts.code + counts.doc + counts.comment + counts.blank;
}

/**
 * Measures a generated directory: `{ platform: { kind: counts, total: counts } }`.
 * The manifest (`.undra-generated`) is not part of any tree.
 */
export function measure(root) {
  const result = {};
  for (const { dir, name } of PLATFORMS) {
    const tree = join(root, dir);
    const kinds = { total: ZERO() };
    for (const path of walk(tree)) {
      if (basename(path) === ".gitattributes") continue; // the tree's own provenance file, not bindings
      const counts = classify(readFileSync(path, "utf8"), path);
      const kind = kindOf(path);
      add((kinds[kind] ??= ZERO()), counts);
      add(kinds.total, counts);
    }
    result[name] = kinds;
  }
  return result;
}

const pct = (part, whole) => (whole ? `${Math.round((100 * part) / whole)}%` : "-");

/** The measurement as the Markdown tables of the docs and the decision record. */
export function markdown(measured) {
  const out = [];
  for (const [platform, kinds] of Object.entries(measured)) {
    out.push(`**${platform}**`, "", "| Kind | Files | Lines | Code | Doc comments | Other comments | Blank |", "|---|---:|---:|---:|---:|---:|---:|");
    const names = Object.keys(kinds).filter((k) => k !== "total" && k !== "package").sort();
    if (kinds.package) names.push("package");
    for (const kind of [...names, "total"]) {
      const c = kinds[kind];
      const label = kind === "total" ? "**Total**" : kind === "package" ? "Package and build files" : kind;
      const cell = (n) => (kind === "total" ? `**${n.toLocaleString("en-US")}**` : n.toLocaleString("en-US"));
      out.push(
        `| ${label} | ${cell(c.files)} | ${cell(c.lines)} | ${cell(c.code)} | ${cell(c.doc)} | ${cell(c.comment)} | ${cell(c.blank)} |`,
      );
    }
    const t = kinds.total;
    out.push("", `Shares of ${platform}: code ${pct(t.code, t.lines)}, doc comments ${pct(t.doc, t.lines)}, other comments ${pct(t.comment, t.lines)}, blank ${pct(t.blank, t.lines)}.`, "");
  }
  return out.join("\n");
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const json = args.includes("--json");
  const root = args.find((a) => !a.startsWith("--")) ?? "examples/playground/generated";
  const measured = measure(root);
  if (json) console.log(JSON.stringify(measured, null, 2));
  else {
    console.log(`Generated tree: ${relative(process.cwd(), root) || root}\n`);
    console.log(markdown(measured));
  }
}
