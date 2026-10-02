// Measurement-only Vite plugins for the 16 KB design (ADR-057). They reproduce, inside the gate's build, what the
// publish-time passes would do to `dist`, so every lever can be measured on one tree.
import { readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const RESERVED = ["_set", "_signals", "_apply", "_observeAll"]; // what generated code names (SPEC 17.1)

function sources(dir, out = []) {
  for (const f of readdirSync(dir)) {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) sources(p, out);
    else if (p.endsWith(".ts") && !p.endsWith(".d.ts")) out.push(p);
  }
  return out;
}

/** Lever (a): every `_name` property of the runtime's own modules becomes `_a`, `_b`, ... through one shared cache. */
export async function mangle(runtime, { style = "under", extra = [], dump } = {}) {
  const { minifySync } = await import(pathToFileURL(join(runtime, "node_modules/rolldown/dist/utils-index.mjs")).href);
  const want = (n) => (/^_[A-Za-z]/.test(n) || extra.includes(n)) && !RESERVED.includes(n);
  const freq = new Map();
  for (const f of sources(join(runtime, "src")))
    for (const m of readFileSync(f, "utf8").matchAll(/(?<![\w$])([A-Za-z_$][\w$]*)/g)) if (want(m[1])) freq.set(m[1], (freq.get(m[1]) ?? 0) + 1);
  const names = [...freq.entries()].sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1)).map((e) => e[0]);
  const alphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
  const short = (i) => {
    let s = "";
    do {
      s = alphabet[i % 52] + s;
      i = Math.floor(i / 52) - 1;
    } while (i >= 0);
    return s;
  };
  let cache = {};
  if (style === "under") names.forEach((n, i) => (cache[n] = `_${short(i)}`));
  const include = new RegExp(`^(_[A-Za-z].*${extra.length ? "|" + extra.join("|") : ""})$`);
  return {
    name: "proto-mangle-props",
    enforce: "post",
    transform(code, id) {
      if (!id.startsWith(runtime + "/src/") && !id.startsWith(runtime + "/dist/")) return null;
      const r = minifySync(id, code, { module: true, compress: false, mangle: false, mangleProps: { include, reserved: RESERVED, cache }, codegen: { removeWhitespace: false } });
      if (r.errors?.length) throw new Error(`${id}: ${JSON.stringify(r.errors[0])}`);
      cache = r.mangleCache ?? cache;
      return { code: r.code, map: null };
    },
    closeBundle() {
      if (dump) writeFileSync(dump, JSON.stringify(cache, null, 1));
    },
  };
}

/**
 * Lever (b): every human-readable message of the runtime (a string or template with at least two spaces, outside types)
 * becomes `__m(code, ...interpolated values)`: what a production build that keeps codes and arguments up front, and the
 * sentences in a table loaded on demand, would ship. `keep` lists substrings of messages that stay as they are.
 */
export function messages(runtime, { helper, dump, minSpaces = 2, keep = [] } = {}) {
  const require = createRequire(join(runtime, "package.json"));
  const ts = require("typescript");
  const table = [];
  return {
    name: "proto-message-codes",
    enforce: "pre",
    transform(code, id) {
      if (!(id.startsWith(runtime + "/src/") || id.startsWith(runtime + "/dist/")) || id === helper) return null;
      const sf = ts.createSourceFile(id, code, ts.ScriptTarget.Latest, true);
      const edits = [];
      const visit = (node) => {
        if (ts.isTypeNode(node) || ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) return;
        if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node) || ts.isTemplateExpression(node)) {
          const parent = node.parent;
          const skip =
            ts.isCaseClause(parent) || ts.isPropertyAssignment(parent) && parent.name === node || ts.isLiteralTypeNode(parent) ||
            ts.isTaggedTemplateExpression(parent) || ts.isEnumMember(parent);
          let text;
          let exprs = [];
          if (ts.isTemplateExpression(node)) {
            text = node.head.text + node.templateSpans.map((s, i) => `{${i}}` + s.literal.text).join("");
            exprs = node.templateSpans.map((s) => s.expression.getText(sf));
          } else text = node.text;
          const spaces = (text.match(/ /g) ?? []).length;
          if (!skip && spaces >= minSpaces && !keep.some((k) => text.includes(k))) {
            const n = table.push(text);
            edits.push([node.getStart(sf), node.end, `__m(${n}${exprs.map((e) => `, ${e}`).join("")})`]);
            return; // nested templates inside the expressions are dropped with it: they were arguments' own text
          }
        }
        ts.forEachChild(node, visit);
      };
      visit(sf);
      if (edits.length === 0) return null;
      let out = code;
      for (const [s, e, r] of edits.sort((a, b) => b[0] - a[0])) out = out.slice(0, s) + r + out.slice(e);
      return { code: `import { __m } from ${JSON.stringify(helper)};\n` + out, map: null };
    },
    closeBundle() {
      if (dump) writeFileSync(dump, JSON.stringify(table, null, 1));
    },
  };
}
