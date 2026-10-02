#!/usr/bin/env node
// Where the bytes of the JavaScript size gate are (ADR-052 section 5 says how to do this for the wasm; this is the same
// for `web/hello-runtime-js`, ADR-057). It builds the hello app's chunks exactly as the gate does
// (scripts/web-size-runtime.mjs, with source maps beside them) and attributes every byte of the runtime's chunk, and
// every bit of its deflate stream, to the source module, the declaration (a function, a class member, a top-level
// statement) and the concern it came from.
//
//   scripts/wasm-size.sh                                  # once: makes target/wasm-size/hello
//   node scripts/web-size-attribute.mjs                   # per module and per concern
//   node scripts/web-size-attribute.mjs --symbols[=N]     # also per declaration (the N largest)
//   node scripts/web-size-attribute.mjs --strings[=N]     # also every string of the chunk (the N costliest)
//   node scripts/web-size-attribute.mjs --project <dir> --runtime <dir> --out <dir> --json <file>
//
// Method.
// 1. Minified bytes. The chunk's source map gives, for every span of the minified text, the source file, line and column
//    it was printed from. What has no mapping is the bundler's own: the chunk's table of on-demand chunks
//    (`__vite__mapDeps`) and Vite's preload helper, a virtual module the build adds wherever a chunk has an `import()`.
//    The chunk's closing `export { .. }` list is charged to the last declaration before it (the store base class).
// 2. Gzip share. The chunk is compressed as the gate compresses it (Python's zlib, level 9) and the deflate stream is
//    decoded here symbol by symbol: a literal costs its Huffman code's bits; a match costs its length and distance codes
//    and their extra bits, spread evenly over the bytes it produces; the block headers, the code tables and the gzip
//    framing are spread over every byte in proportion. The per-byte costs add up to the compressed size exactly (checked).
// 3. What this is not. A back-reference is charged to the bytes it produces, not to the text it copies from, so a share
//    is what the code costs *in this chunk*: removing it can save less (its text was a dictionary for other code) or
//    more. A lever's worth is therefore the difference between two builds measured by the gate, never a row of this
//    table; the table says where to look.
//
// The concerns are rules over (module, declaration) in scripts/web-size-concerns.mjs; a declaration no rule names is
// listed as unassigned, so the rules cannot go stale quietly.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { classify } from "./web-size-concerns.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
/** `--name value`, `--name=value` or a bare `--name` (true); `undefined` when absent. */
function option(name) {
  const at = args.findIndex((a) => a === `--${name}` || a.startsWith(`--${name}=`));
  if (at < 0) return undefined;
  const arg = args[at];
  if (arg.includes("=")) return arg.slice(arg.indexOf("=") + 1);
  const next = args[at + 1];
  return next !== undefined && !next.startsWith("--") && ["project", "runtime", "out", "json"].includes(name) ? next : true;
}
const project = resolve(String(option("project") ?? join(ROOT, "target", "wasm-size", "hello")));
const runtimeDir = resolve(String(option("runtime") ?? join(ROOT, "runtimes", "ts", "@undra", "runtime")));
const out = resolve(String(option("out") ?? join(ROOT, "target", "wasm-size", "js-attribute")));
if (!existsSync(join(project, "generated", "ts", "package.json"))) {
  console.error(`web-size-attribute: ${project} is not a built hello project; run scripts/wasm-size.sh first (or pass --project)`);
  process.exit(2);
}

// ----- the gate's build, with source maps ----------------------------------------------------------------------------
const built = JSON.parse(
  execFileSync(process.execPath, [join(ROOT, "scripts", "web-size-runtime.mjs"), project, runtimeDir, out], {
    env: { ...process.env, UNDRA_SIZE_SOURCEMAP: "1" },
    stdio: ["ignore", "pipe", "inherit"],
  }).toString(),
);
const chunkPath = join(out, built.runtime);
const require = createRequire(join(runtimeDir, "package.json"));
const ts = require("typescript");
const acorn = require("acorn");
const { TraceMap, decodedMappings } = require("@jridgewell/trace-mapping");

const code = readFileSync(chunkPath);
const text = code.toString("latin1"); // one character per byte: offsets below are byte offsets
const gzBytes = execFileSync(
  "python3",
  ["-c", "import gzip,sys;sys.stdout.buffer.write(gzip.compress(open(sys.argv[1],'rb').read(),9,mtime=0))", chunkPath],
  { maxBuffer: 1 << 28 },
);

// ----- 1. what every byte costs in the deflate stream ----------------------------------------------------------------
/** The compressed cost, in bytes, of each of the `n` bytes that `gz` (a gzip stream without optional header fields) inflates to. */
function inflateCosts(gz, n) {
  let p = 10 * 8; // past the gzip header
  const bit = () => {
    const b = (gz[p >> 3] >> (p & 7)) & 1;
    p++;
    return b;
  };
  const bits = (k) => {
    let v = 0;
    for (let i = 0; i < k; i++) v |= bit() << i;
    return v;
  };
  /** The canonical Huffman code of `lengths`, as a map from "length:code" to symbol. */
  const build = (lengths) => {
    const count = new Array(16).fill(0);
    for (const l of lengths) count[l]++;
    count[0] = 0;
    const next = new Array(16).fill(0);
    let c = 0;
    for (let i = 1; i < 16; i++) {
      c = (c + count[i - 1]) << 1;
      next[i] = c;
    }
    const table = new Map();
    lengths.forEach((l, symbol) => {
      if (l > 0) table.set(`${l}:${next[l]++}`, symbol);
    });
    return table;
  };
  const read = (table) => {
    let c = 0;
    for (let l = 1; l < 16; l++) {
      c = (c << 1) | bit();
      const symbol = table.get(`${l}:${c}`);
      if (symbol !== undefined) return symbol;
    }
    throw new Error("web-size-attribute: bad Huffman code in the deflate stream");
  };
  const LENGTH_BASE = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
  const LENGTH_EXTRA = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
  const DISTANCE_EXTRA = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
  const cost = new Float64Array(n);
  let o = 0;
  let overhead = 0;
  for (let final = 0; !final; ) {
    const start = p;
    final = bit();
    const type = bits(2);
    if (type === 0) {
      p = (p + 7) & ~7;
      const len = bits(16);
      bits(16);
      overhead += p - start;
      for (let i = 0; i < len; i++) cost[o++] = 8;
      p += len * 8;
      continue;
    }
    let literals;
    let distances;
    if (type === 1) {
      const fixed = [];
      for (let i = 0; i < 288; i++) fixed.push(i < 144 ? 8 : i < 256 ? 9 : i < 280 ? 7 : 8);
      literals = build(fixed);
      distances = build(new Array(30).fill(5));
    } else {
      const hlit = bits(5) + 257;
      const hdist = bits(5) + 1;
      const hclen = bits(4) + 4;
      const order = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
      const codeLengths = new Array(19).fill(0);
      for (let i = 0; i < hclen; i++) codeLengths[order[i]] = bits(3);
      const lengthCode = build(codeLengths);
      const lengths = [];
      while (lengths.length < hlit + hdist) {
        const symbol = read(lengthCode);
        if (symbol < 16) lengths.push(symbol);
        else if (symbol === 16) {
          const previous = lengths[lengths.length - 1];
          for (let r = bits(2) + 3; r > 0; r--) lengths.push(previous);
        } else for (let r = symbol === 17 ? bits(3) + 3 : bits(7) + 11; r > 0; r--) lengths.push(0);
      }
      literals = build(lengths.slice(0, hlit));
      distances = build(lengths.slice(hlit));
    }
    overhead += p - start;
    for (;;) {
      const before = p;
      const symbol = read(literals);
      if (symbol === 256) {
        overhead += p - before;
        break;
      }
      if (symbol < 256) {
        cost[o++] = p - before;
        continue;
      }
      const len = LENGTH_BASE[symbol - 257] + bits(LENGTH_EXTRA[symbol - 257]);
      bits(DISTANCE_EXTRA[read(distances)]);
      const each = (p - before) / len;
      for (let i = 0; i < len; i++) cost[o++] = each;
    }
  }
  if (o !== n) throw new Error(`web-size-attribute: the stream inflates to ${o} bytes, the chunk has ${n}`);
  overhead += gz.length * 8 - 8 * 8 - p; // the padding of the last byte
  overhead += 18 * 8; // the gzip header and trailer
  let sum = 0;
  for (const c of cost) sum += c;
  const scale = (sum + overhead) / sum;
  for (let i = 0; i < n; i++) cost[i] = (cost[i] * scale) / 8;
  if (Math.abs((sum + overhead) / 8 - gz.length) > 0.01) throw new Error("web-size-attribute: the per-byte costs do not add up to the compressed size");
  return { cost, overheadBytes: overhead / 8 };
}
const { cost, overheadBytes } = inflateCosts(gzBytes, code.length);

// ----- 2. which source position every byte was printed from ----------------------------------------------------------
const map = new TraceMap(JSON.parse(readFileSync(`${chunkPath}.map`, "utf8")));
const sources = map.sources.map((source) => {
  const abs = resolve(dirname(chunkPath), source);
  const inRuntime = /runtimes[\\/]ts[\\/]@undra[\\/]runtime[\\/](.*)$/.exec(abs);
  return { abs, short: inRuntime ? inRuntime[1] : source.replace(/^(\.\.\/)+/, "") };
});
const lineStart = [0];
for (let i = 0; i < text.length; i++) if (text.charCodeAt(i) === 10) lineStart.push(i + 1);
const ownerSource = new Int32Array(code.length).fill(-1);
const ownerLine = new Int32Array(code.length);
const ownerColumn = new Int32Array(code.length);
// Columns of a source map are UTF-16 units; the runtime's chunk is ASCII apart from what a string literal holds, so a
// line with a wider character is mapped approximately (a few bytes may move to the neighbouring declaration).
decodedMappings(map).forEach((segments, line) => {
  const base = lineStart[line];
  const end = line + 1 < lineStart.length ? lineStart[line + 1] : text.length;
  segments.forEach((segment, i) => {
    if (segment.length < 4) return;
    const to = Math.min(end, i + 1 < segments.length ? base + segments[i + 1][0] : end);
    for (let k = base + segment[0]; k < to; k++) {
      ownerSource[k] = segment[1];
      ownerLine[k] = segment[2];
      ownerColumn[k] = segment[3];
    }
  });
});
// Vite's preload helper has no mapping, so its text would be charged to the mapped token before it: find it by the
// string it starts with and give it a name of its own.
const VITE_HELPER = -2;
{
  const mark = text.indexOf("`modulepreload`");
  if (mark >= 0) {
    const from = text.lastIndexOf(",", mark);
    let to = mark;
    while (to < code.length && ownerSource[to] === ownerSource[mark] && ownerLine[to] === ownerLine[mark] && ownerColumn[to] === ownerColumn[mark]) to++;
    for (let k = from; k < to; k++) ownerSource[k] = VITE_HELPER;
  }
}

// ----- 3. which declaration a source position is in ------------------------------------------------------------------
const declarationIndex = new Map();
/** For one source file: a function from (line, column) to the name of the enclosing declaration. */
function indexSource(sourceIndex) {
  const { abs } = sources[sourceIndex];
  let source;
  try {
    source = readFileSync(abs, "utf8");
  } catch {
    return () => "(module)";
  }
  const file = ts.createSourceFile(abs, source, ts.ScriptTarget.Latest, true);
  const spans = []; // [start, end, name]; a later span inside an earlier one wins
  const nameOf = (node) => (node.name && node.name.getText ? node.name.getText(file) : undefined);
  const classSpans = (node, name) => {
    spans.push([node.getStart(file), node.end, `${name}.(fields)`]);
    for (const member of node.members) {
      const memberName = ts.isConstructorDeclaration(member) ? "constructor" : (nameOf(member) ?? "(member)");
      const isField =
        ts.isPropertyDeclaration(member) && !(member.initializer && (ts.isArrowFunction(member.initializer) || ts.isObjectLiteralExpression(member.initializer)));
      spans.push([member.getStart(file), member.end, isField ? `${name}.(fields)` : `${name}.${memberName}`]);
    }
  };
  for (const statement of file.statements) {
    if (ts.isClassDeclaration(statement)) classSpans(statement, nameOf(statement) ?? "(class)");
    else if (ts.isFunctionDeclaration(statement)) spans.push([statement.getStart(file), statement.end, nameOf(statement) ?? "(function)"]);
    else if (ts.isVariableStatement(statement)) {
      for (const declaration of statement.declarationList.declarations) spans.push([statement.getStart(file), statement.end, declaration.name.getText(file)]);
    } else if (ts.isEnumDeclaration(statement)) spans.push([statement.getStart(file), statement.end, nameOf(statement) ?? "(enum)"]);
    else if (ts.isModuleDeclaration(statement)) {
      spans.push([statement.getStart(file), statement.end, nameOf(statement) ?? "(namespace)"]);
      if (statement.body && ts.isModuleBlock(statement.body)) {
        for (const inner of statement.body.statements) {
          if (ts.isClassDeclaration(inner)) spans.push([inner.getStart(file), inner.end, `${nameOf(statement)}.${nameOf(inner)}`]);
        }
      }
    } else if (ts.isImportDeclaration(statement) || ts.isExportDeclaration(statement)) spans.push([statement.getStart(file), statement.end, "(imports)"]);
    else spans.push([statement.getStart(file), statement.end, "(module)"]);
  }
  const lines = file.getLineStarts();
  return (line, column) => {
    const at = lines[Math.min(line, lines.length - 1)] + column;
    let name = "(module)";
    for (const [start, end, spanName] of spans) if (at >= start && at < end) name = spanName;
    return name;
  };
}
const declarationOf = (sourceIndex, line, column) => {
  if (!declarationIndex.has(sourceIndex)) declarationIndex.set(sourceIndex, indexSource(sourceIndex));
  return declarationIndex.get(sourceIndex)(line, column);
};

// ----- 4. the strings of the chunk (the minified text, parsed) --------------------------------------------------------
const isString = new Uint8Array(code.length);
const strings = [];
{
  const walk = (node) => {
    if (!node || typeof node.type !== "string") return;
    if (node.type === "Literal" && typeof node.value === "string") strings.push([node.start, node.end]);
    else if (node.type === "TemplateElement" && node.end > node.start) strings.push([node.start, node.end]);
    for (const key of Object.keys(node)) {
      const value = node[key];
      if (Array.isArray(value)) value.forEach(walk);
      else if (value && typeof value.type === "string") walk(value);
    }
  };
  walk(acorn.parse(text, { ecmaVersion: "latest", sourceType: "module" }));
  for (const [start, end] of strings) for (let i = start; i < end; i++) isString[i] = 1;
}

// ----- 5. the tables --------------------------------------------------------------------------------------------------
const add = (table, key, i) => {
  const row = table.get(key) ?? { bytes: 0, gz: 0, stringBytes: 0, stringGz: 0 };
  row.bytes++;
  row.gz += cost[i];
  if (isString[i]) {
    row.stringBytes++;
    row.stringGz += cost[i];
  }
  table.set(key, row);
};
const perModule = new Map();
const perDeclaration = new Map();
const perConcern = new Map();
const unassigned = new Map();
const keyOf = new Array(code.length);
for (let i = 0; i < code.length; i++) {
  const source = ownerSource[i];
  const module = source === VITE_HELPER ? "(vite preload helper)" : source < 0 ? "(bundler: the chunk's table of on-demand chunks)" : sources[source].short;
  const declaration = source < 0 ? "" : declarationOf(source, ownerLine[i], ownerColumn[i]);
  keyOf[i] = [module, declaration];
  add(perModule, module, i);
  add(perDeclaration, `${module}  ${declaration}`, i);
  const concern = classify(module, declaration);
  add(perConcern, concern ?? "(unassigned)", i);
  if (concern === undefined) add(unassigned, `${module}  ${declaration}`, i);
}
const pad = (n) => String(Math.round(n)).padStart(7);
const print = (title, table, limit) => {
  console.log(`\n${title}\n  bytes     gz  str.b str.gz  name`);
  const rows = [...table.entries()].sort((a, b) => b[1].gz - a[1].gz);
  for (const [key, row] of rows.slice(0, limit ?? rows.length)) console.log(`${pad(row.bytes)}${pad(row.gz)}${pad(row.stringBytes)}${pad(row.stringGz)}  ${key}`);
};
const count = (value) => (value === true || value === undefined ? undefined : Number(value));
console.log(`${built.runtime}: ${code.length} bytes, ${gzBytes.length} gzipped (zlib level 9; ${overheadBytes.toFixed(0)} bytes of tables and framing, spread)`);
print("per concern", perConcern);
if (unassigned.size > 0) print("declarations no rule of scripts/web-size-concerns.mjs names", unassigned, 40);
print("per module", perModule);
if (option("symbols") !== undefined) print("per declaration", perDeclaration, count(option("symbols")));

let stringBytes = 0;
let stringGz = 0;
for (let i = 0; i < code.length; i++) {
  if (!isString[i]) continue;
  stringBytes++;
  stringGz += cost[i];
}
console.log(`\nstrings: ${strings.length} (string literals and the text parts of templates), ${stringBytes} bytes, ${Math.round(stringGz)} gzipped`);
if (option("strings") !== undefined) {
  const rows = strings.map(([start, end]) => {
    let gz = 0;
    for (let i = start; i < end; i++) gz += cost[i];
    return [end - start, gz, text.slice(start, end), keyOf[start]];
  });
  rows.sort((a, b) => b[1] - a[1]);
  for (const [bytes, gz, value, key] of rows.slice(0, count(option("strings")) ?? rows.length)) {
    console.log(`${pad(bytes)}${pad(gz)}  ${key[0]} ${key[1]}  ${value.length > 140 ? `${value.slice(0, 137)}...` : value}`);
  }
}
if (typeof option("json") === "string") {
  const plain = (table) => Object.fromEntries([...table.entries()].map(([key, row]) => [key, { bytes: row.bytes, gz: Math.round(row.gz), stringBytes: row.stringBytes, stringGz: Math.round(row.stringGz) }]));
  writeFileSync(resolve(option("json")), `${JSON.stringify({ chunk: built.runtime, bytes: code.length, gzipped: gzBytes.length, concerns: plain(perConcern), modules: plain(perModule), declarations: plain(perDeclaration) }, null, 1)}\n`);
}
