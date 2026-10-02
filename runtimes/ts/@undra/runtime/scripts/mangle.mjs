// The production build's rename pass (ADR-057, D2): every private property of the runtime is shortened, through one cache for the whole
// package, in the `.js` files `tsc` wrote.
//
// "Private" is SPEC 17.1's rule: a name that starts with an underscore (and a letter) is not API. A name is renamed when this package
// declares it (a class member, an object literal's key, or a property it assigns); a name that is only ever read (`exports._initialize` of a
// WebAssembly module, `module._sqlite3_malloc` of wa-sqlite) belongs to someone else and stays. Four names are the runtime's contract with
// generated code (`_set`, `_signals`, `_apply`, `_observeAll`, SPEC 17.1) and stay too.
//
// The pass edits the text of `tsc`'s output at the exact spans of those property names and changes nothing else, so comments (`/* @__PURE__ */`
// among them), line structure and the source maps stay as `tsc` wrote them; a map's columns move by what an edit added or removed before them.
//
// Names become `_a`, `_b`, .. in the order of how often the package says them, so the most used get the shortest. They keep their underscore:
// a generated member never starts with one (SPEC 17.1), so a renamed runtime member cannot meet a signal called `a`.
import { readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parseSync } from "rolldown/utils";
import { decode, encode } from "@jridgewell/sourcemap-codec";

/** What generated code names (SPEC 17.1): these keep their names. */
export const RESERVED = Object.freeze(["_set", "_signals", "_apply", "_observeAll"]);

/** A private property name: one underscore, then a letter. (`__proto__`, `_` and `_1` are never renamed.) */
const PRIVATE = /^_[A-Za-z$][\w$]*$/;

/** The files of `dir` (recursively) with this suffix. */
function walk(dir, suffix, skip = () => false, base = dir) {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry);
    if (skip(path.slice(base.length + 1))) return [];
    return statSync(path).isDirectory() ? walk(path, suffix, skip, base) : path.endsWith(suffix) ? [path] : [];
  });
}

/** `_a`, `_b`, .. `_z`, `_A`, .. `_Z`, `_aa`, `_ab`, .. */
function shortName(index) {
  const alphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
  let name = "";
  let n = index;
  do {
    name = alphabet[n % alphabet.length] + name;
    n = Math.floor(n / alphabet.length) - 1;
  } while (n >= 0);
  return `_${name}`;
}

/** Whether an ESTree value is a node. */
const isNode = (value) => value !== null && typeof value === "object" && typeof value.type === "string";

/**
 * Every property occurrence of a module, as `{ name, start, end, role }`:
 * `declared` (a class member, an object literal's or pattern's key, the target of an assignment) or `read`; `shorthand` marks `{ _x }`,
 * whose key is also a variable, so a rename has to say `{ _a: _x }`.
 */
export function propertyOccurrences(file, source) {
  const parsed = parseSync(file, source, { sourceType: "module", lang: "js" });
  if (parsed.errors.length > 0) throw new Error(`mangle: ${file} does not parse: ${parsed.errors[0].message}`);
  const found = [];
  const exportedNames = [];
  const strings = new Set();
  const note = (node, role, shorthand = false) => found.push({ name: node.name, start: node.start, end: node.end, role, shorthand });
  const visit = (node, parent) => {
    switch (node.type) {
      case "MemberExpression":
        if (!node.computed && node.property.type === "Identifier") {
          const assigned = parent && parent.type === "AssignmentExpression" && parent.left === node;
          note(node.property, assigned ? "declared" : "read");
        }
        break;
      case "PropertyDefinition":
      case "MethodDefinition":
      case "AccessorProperty":
        if (!node.computed && node.key.type === "Identifier") note(node.key, "declared");
        break;
      case "Property":
        if (!node.computed && node.key.type === "Identifier") {
          // Of an object literal a key is a declaration; of a destructuring pattern it is a read of someone's property.
          const pattern = parent && parent.type === "ObjectPattern";
          note(node.key, pattern ? "read" : "declared", node.shorthand);
        }
        break;
      case "Literal":
        if (typeof node.value === "string" && PRIVATE.test(node.value)) strings.add(node.value);
        break;
      case "ExportNamedDeclaration":
        for (const specifier of node.specifiers ?? []) exportedNames.push(specifier.exported.name ?? specifier.exported.value);
        break;
      case "ImportSpecifier":
        exportedNames.push(node.imported.name ?? node.imported.value);
        break;
      default:
    }
    for (const key of Object.keys(node)) {
      const child = node[key];
      if (Array.isArray(child)) {
        for (const item of child) if (isNode(item)) visit(item, node);
      } else if (isNode(child)) visit(child, node);
    }
  };
  visit(parsed.program, null);
  // `export const _x` / `export function _x` / `export class _x`: a module binding that is a name too.
  for (const statement of parsed.program.body) {
    const declaration = statement.type === "ExportNamedDeclaration" ? statement.declaration : null;
    if (!declaration) continue;
    if (declaration.id) exportedNames.push(declaration.id.name);
    for (const d of declaration.declarations ?? []) if (d.id.type === "Identifier") exportedNames.push(d.id.name);
  }
  return { found, exportedNames, strings };
}

/**
 * Plans the rename of every `.js` file of `dist` (the files `skip` leaves out are not read): the cache, and what stays and why.
 * Throws when a name would be renamed and also be a module's export name, or a string literal of its own (a rename cannot reach those).
 */
export function planRename(dist, { skip = () => false } = {}) {
  const files = walk(dist, ".js", skip);
  const modules = new Map();
  const declared = new Map();
  const reads = new Map();
  const exported = new Set();
  for (const file of files) {
    const source = readFileSync(file, "utf8");
    const { found, exportedNames, strings } = propertyOccurrences(file, source);
    modules.set(file, { source, found, strings });
    for (const name of exportedNames) exported.add(name);
    for (const { name, role } of found) {
      if (!PRIVATE.test(name)) continue;
      const table = role === "declared" ? declared : reads;
      table.set(name, (table.get(name) ?? 0) + 1);
    }
  }
  const renamed = [...declared.keys()].filter((name) => !RESERVED.includes(name));
  const clash = renamed.filter((name) => exported.has(name));
  if (clash.length > 0) throw new Error(`mangle: ${clash.join(", ")} is both a private property and the name of an export: a rename cannot reach the export`);
  // A string that is exactly a renamed name (`target["_pending"]`, `"_pending" in o`) would keep meaning the old name: the build fails on it.
  for (const [file, { strings }] of modules) {
    const quoted = [...strings].filter((text) => renamed.includes(text));
    if (quoted.length > 0) throw new Error(`mangle: ${file} says ${quoted.map((q) => JSON.stringify(q)).join(", ")} as a string, and the package renames a property of that name: write it as a property, or rename it`);
  }
  const uses = (name) => (declared.get(name) ?? 0) + (reads.get(name) ?? 0);
  renamed.sort((a, b) => uses(b) - uses(a) || (a < b ? -1 : 1));
  // What the package says as a property and does not own, and what generated code names: output names never equal these.
  const kept = new Set([...reads.keys(), ...RESERVED, "_resync"].filter((name) => !renamed.includes(name)));
  const cache = {};
  let next = 0;
  for (const name of renamed) {
    let short = shortName(next++);
    while (kept.has(short) || declared.has(short) || reads.has(short)) short = shortName(next++);
    cache[name] = short;
  }
  // Names the package reads and never declares, in order: someone else's (the test holds that list).
  const foreign = [...reads.keys()].filter((name) => !declared.has(name) && !RESERVED.includes(name)).sort();
  return { files, modules, cache, foreign };
}

/** Applies `cache` to one module's source: the edited text, and the edits as `{ start, end, text }` in order. */
export function renameSource(source, found, cache) {
  const edits = [];
  for (const { name, start, end, shorthand } of found) {
    if (!Object.hasOwn(cache, name)) continue; // (`constructor` and `toString` are not names of the cache)
    const to = cache[name];
    edits.push({ start, end, text: shorthand ? `${to}: ${name}` : to });
  }
  edits.sort((a, b) => a.start - b.start);
  let out = "";
  let at = 0;
  for (const edit of edits) {
    out += source.slice(at, edit.start) + edit.text;
    at = edit.end;
  }
  return { code: out + source.slice(at), edits };
}

/** The line start offsets of a text. */
function lineStarts(source) {
  const starts = [0];
  for (let i = 0; i < source.length; i++) if (source.charCodeAt(i) === 10) starts.push(i + 1);
  return starts;
}

/** Moves a source map's generated columns by what `edits` added or removed on their lines. */
export function shiftMap(map, source, edits) {
  const starts = lineStarts(source);
  const lineOf = (offset) => {
    let lo = 0;
    let hi = starts.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (starts[mid] <= offset) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  };
  const byLine = new Map();
  for (const edit of edits) {
    const line = lineOf(edit.start);
    const column = edit.start - starts[line];
    const list = byLine.get(line) ?? [];
    list.push({ column, end: column + (edit.end - edit.start), delta: edit.text.length - (edit.end - edit.start) });
    byLine.set(line, list);
  }
  const lines = decode(map.mappings);
  lines.forEach((segments, line) => {
    const list = byLine.get(line);
    if (!list) return;
    for (const segment of segments) {
      let shift = 0;
      for (const edit of list) {
        if (edit.end <= segment[0]) shift += edit.delta; // (a segment inside a renamed name keeps the name's start)
      }
      segment[0] += shift;
    }
  });
  return { ...map, mappings: encode(lines) };
}

/**
 * Renames the private properties of every `.js` file of `dist`, in place, with one cache, written to `<dist>/mangle-cache.json`;
 * maps next to the files (`<file>.map`) follow. Returns `{ cache, foreign, files }`.
 */
export function mangleDist(dist, options = {}) {
  const plan = planRename(dist, options);
  let changed = 0;
  for (const file of plan.files) {
    const { source, found } = plan.modules.get(file);
    const { code, edits } = renameSource(source, found, plan.cache);
    if (edits.length === 0) continue;
    writeFileSync(file, code);
    changed++;
    try {
      const map = JSON.parse(readFileSync(`${file}.map`, "utf8"));
      writeFileSync(`${file}.map`, JSON.stringify(shiftMap(map, source, edits)));
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }
  writeFileSync(join(dist, "mangle-cache.json"), `${JSON.stringify(plan.cache, null, 1)}\n`);
  return { cache: plan.cache, foreign: plan.foreign, files: changed };
}
