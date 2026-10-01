// A declarations-only view of the code `undra bindgen` writes: for Swift, Kotlin and TypeScript, the doc comments
// and the signatures of everything an app can see, without bodies, initializers or plumbing.
//
//   import { declarations } from "./decls.mjs";
//   const { text, names } = declarations(source, "swift");
//
// This is not a compiler front end. It reads the one shape bindgen emits (SPEC section 10, the goldens under
// crates/undra-bindgen/tests/golden): braces and parentheses balance, strings and comments are masked before
// anything is counted, a declaration starts on its own line. What it keeps and what it drops:
//
//   kept     every declaration an app can see: types, enums and their cases, errors, records and their fields,
//            methods, initializers, properties, ports, ids; each with the doc comment above it, verbatim.
//   dropped  function and initializer bodies, property initializers (a literal constant keeps its value), imports,
//            private/internal/protected members, `override` members, `init {}` blocks, ordinary comments.
//   folded   a run of six or more declarations that differ only in a number (the benchmark store's 128 counters
//            s000 ... s127) keeps its first and last line and says how many it skipped.
//
// What it does not understand (a shape bindgen does not emit today; support it with a rule and a test in
// decls.test.mjs the day bindgen does):
//   * visibility is read from modifiers alone: Swift keeps what says `public` or `open` (and enum cases, protocol
//     requirements), Kotlin drops `private`, `internal` and `protected`, TypeScript keeps what is exported and not
//     private or protected;
//   * an initializer that is itself a block (`val x: T = run { ... }`, `let x: T = { ... }()`) loses its value but a
//     `get() { ... }` accessor, a Swift `where` clause on its own line, `#if`, TypeScript overloads and a return type
//     that is an object literal are not read at all;
//   * a property with no declared type keeps its initializer (the value is all there is to show);
//   * the fold is by the shape of the line, not by meaning, and only for consecutive lines.
//
// Node 20+, no dependencies.

const LANGS = new Set(["swift", "kotlin", "ts"]);
/** A run of this many lines that are alike except for digits is folded. */
export const FOLD_AT = 6;

// ---------------------------------------------------------------------------------------------------------------
// Masking: the same text with comments and string contents blanked (newlines and quotes kept), so that counting
// braces and looking for `=` or `;` never trips over text inside a string or a comment.

/** `src` with comments and the insides of string literals replaced by spaces. */
export function mask(src, lang) {
  const out = src.split("");
  const n = src.length;
  const blank = (a, b) => { for (let k = a; k < b; k++) if (out[k] !== "\n") out[k] = " "; };
  const lineEnd = (i) => { const e = src.indexOf("\n", i); return e < 0 ? n : e; };

  /** Scans code from `i` to the `close` that matches an already consumed `open` (or to the end when `open` is null); returns the index after it. */
  function code(i, open, close) {
    let depth = 0;
    while (i < n) {
      const c = src[i], d = src[i + 1];
      if (c === "/" && d === "/") { const e = lineEnd(i); blank(i, e); i = e; continue; }
      if (c === "/" && d === "*") { const e = src.indexOf("*/", i + 2); const stop = e < 0 ? n : e + 2; blank(i, stop); i = stop; continue; }
      if (c === '"' || (c === "'" && lang !== "swift") || (c === "`" && lang === "ts")) { i = string(i); continue; }
      if (open) {
        if (c === open) depth++;
        else if (c === close) { if (depth === 0) return i + 1; depth--; }
      }
      i++;
    }
    return n;
  }

  /** Scans the string literal that starts at `i`; returns the index after it. Its contents are blanked. */
  function string(i) {
    const q = src[i];
    if (q !== "'" && src.startsWith(q.repeat(3), i)) { // a multi-line string
      const e = src.indexOf(q.repeat(3), i + 3); const stop = e < 0 ? n : e + 3;
      blank(i + 3, stop - 3); return stop;
    }
    let j = i + 1;
    while (j < n) {
      const c = src[j];
      if (c === "\\") {
        if (lang === "swift" && src[j + 1] === "(") { j = code(j + 2, "(", ")"); continue; } // "\(interpolation)"
        j += 2; continue;
      }
      if (c === "$" && src[j + 1] === "{" && lang !== "swift") { j = code(j + 2, "{", "}"); continue; } // "${interpolation}"
      if (c === q) { blank(i + 1, j); return j + 1; }
      if (c === "\n" && q !== "`") { blank(i + 1, j); return j; } // an unterminated string ends at the line
      j++;
    }
    blank(i + 1, n);
    return n;
  }

  code(0, null, null);
  return out.join("");
}

/** The index of the bracket that closes the one at `i` in the masked text `m`. */
function closer(m, i) {
  const open = m[i], close = { "(": ")", "[": "]", "{": "}" }[open];
  let depth = 0;
  for (let j = i; j < m.length; j++) {
    if (m[j] === open) depth++;
    else if (m[j] === close && --depth === 0) return j;
  }
  throw new Error(`unbalanced "${open}" at offset ${i}`);
}

// ---------------------------------------------------------------------------------------------------------------
// Statements: the declarations at one nesting level.

/** Where the first top-level `=` of the masked head is (not `==`, `=>`, `<=`, `>=`, `!=`), or -1. Brackets are skipped. */
function equalsAt(h) {
  for (let i = 0; i < h.length; i++) {
    const c = h[i];
    if (c === "(" || c === "[" || c === "{") { i = closer(h, i); continue; }
    if (c === "=" && h[i + 1] !== "=" && h[i + 1] !== ">" && !"=!<>".includes(h[i - 1] ?? "")) return i;
  }
  return -1;
}
/** Where the first top-level `:` of the masked head is, or -1. */
function colonAt(h) {
  for (let i = 0; i < h.length; i++) {
    const c = h[i];
    if (c === "(" || c === "[" || c === "{") { i = closer(h, i); continue; }
    if (c === ":") return i;
  }
  return -1;
}

/** TypeScript statements whose braces are part of the statement, not a block: type aliases, imports, `export {...}`. */
const tsBalanced = (head) => /^(?:export\s+)?(?:declare\s+)?type\b/.test(head) || /^import\b/.test(head) || /^export\s*\{/.test(head);

/** Does the statement that started at `s` end at the newline at `j`? */
function endsAtNewline(m, s, j, end) {
  const head = m.slice(s, j).trimEnd();
  if (/(?:=|&&|\|\||\+|->|\\)$/.test(head)) return false;
  let k = j; while (k < end && /\s/.test(m[k])) k++;
  return !/^(?:\.|\|\||&&|\?[.?]|:\s|\+)/.test(m.slice(k, k + 3));
}

/**
 * The statements of `m[start, end)`: { s, head, blockStart, blockEnd, tail, gap } where `head` is the end of the text
 * before a `{` block (or of the whole statement), `tail` the end of the statement, and `gap` where the comments above it
 * begin. Nothing in a block is read here.
 */
function statements(m, start, end, lang) {
  const out = [];
  let i = start, prevEnd = start;
  for (;;) {
    while (i < end && /\s/.test(m[i])) i++;
    if (i >= end) break;
    const s = i;
    let j = i, blockStart = -1, blockEnd = -1, tail;
    for (; j < end; j++) {
      const c = m[j];
      if (c === "(" || c === "[") { j = closer(m, j); continue; }
      if (c === "{") {
        if (lang === "ts" && tsBalanced(m.slice(s, j + 1).trim())) { j = closer(m, j); continue; }
        blockStart = j; blockEnd = closer(m, j); break;
      }
      if (c === ";") { j++; break; }
      if (c === "\n" && lang !== "ts" && endsAtNewline(m, s, j, end)) break; // TypeScript statements end at `;` or `}`
    }
    if (blockStart >= 0) {
      tail = blockEnd + 1;
      let k = tail; while (k < end && (m[k] === " " || m[k] === "\t")) k++;
      if (m[k] === "(") { k = closer(m, k) + 1; tail = k; while (k < end && (m[k] === " " || m[k] === "\t")) k++; } // `{ ... }()`
      if (m[k] === ";") tail = k + 1;
      if (lang === "ts" && /^\s*(as\s+const|satisfies\b)/.test(m.slice(tail, tail + 20))) { // `{ ... } as const;`
        const e = m.indexOf(";", tail); tail = e < 0 ? tail : e + 1;
      }
      out.push({ s, head: blockStart, blockStart, blockEnd, tail, gap: prevEnd });
      i = tail;
    } else {
      out.push({ s, head: Math.min(j, end), blockStart: -1, blockEnd: -1, tail: Math.min(j, end), gap: prevEnd });
      i = Math.min(j, end);
    }
    prevEnd = i;
  }
  return out;
}

/** The doc comments in the gap `src[a, b)` before a statement, verbatim, one string per line, and where the first one starts. */
function docsIn(src, a, b, lang) {
  const gap = src.slice(a, b);
  const lines = [];
  let first = -1;
  const re = lang === "swift" ? /^[ \t]*\/\/\/.*$/gm : /^[ \t]*\/\*\*[\s\S]*?\*\//gm;
  // Ordinary comments in the gap hide nothing here: only doc comments are matched.
  for (const m of gap.matchAll(re)) {
    if (first < 0) first = a + m.index;
    lines.push(...m[0].split("\n").map((l) => l.replace(/\s+$/, "")));
  }
  return { lines, first };
}

// ---------------------------------------------------------------------------------------------------------------
// Classification: what a statement is, whether an app can see it, and how to print it.

const squash = (s) => s.replace(/\s+/g, " ").trim();

/**
 * `{ keep, kind, name, container, cut }` for one statement. `h` is the masked head (attributes and modifiers
 * included), `depth` 0 at the top of the file, `parent` the kind of the enclosing container, `block` whether a `{...}` follows.
 */
function classify(lang, h, depth, parent, block) {
  const text = squash(h);
  const no = { keep: false };
  if (lang === "swift") {
    const body = text.replace(/@\w+(?:\([^)]*\))?\s*/g, "");
    const m = /^((?:(?:public|open|internal|private(?:\(set\))?|fileprivate|final|static|lazy|override|convenience|required|nonisolated|mutating|weak|indirect|unowned)\s+)*)(class|struct|enum|extension|protocol|actor|func|init|deinit|subscript|var|let|case|typealias|associatedtype|import)\b\s*([\w.]*)/.exec(body);
    if (!m) return no;
    const [, mods, kw, name] = m;
    if (kw === "import" || /\boverride\b/.test(mods)) return no;
    if (/\b(?:private|fileprivate)\b(?!\(set\))/.test(mods)) return no;
    const visible = /\b(?:public|open)\b/.test(mods) || kw === "case" || parent === "protocol";
    if (kw === "extension") return { keep: true, kind: "extension", name: "", container: true };
    if (!visible) return no;
    if (/^(?:class|struct|enum|protocol|actor)$/.test(kw)) return { keep: true, kind: kw, name, container: true };
    if (kw === "var") return { keep: true, kind: "var", name, cut: block ? "block" : "init" };
    return { keep: true, kind: kw, name, cut: block ? "block" : null };
  }
  if (lang === "kotlin") {
    const body = text.replace(/@[\w.]+(?:\([^)]*\))?\s*/g, "");
    const m = /^((?:(?:public|private|protected|internal|open|abstract|final|override|sealed|data|enum|inner|annotation|const|lateinit|suspend|inline|operator|infix|tailrec|external|expect|actual|vararg|value)\s+)*)(class|interface|object|companion\s+object|fun|val|var|constructor|init|typealias|package|import)\b\s*([\w.]*)/.exec(body);
    if (!m) { // an enum entry: `ALL(0u),`
      return parent === "enum" && /^[A-Z][A-Za-z0-9_]*\b/.test(body) ? { keep: true, kind: "entry", name: /^\w+/.exec(body)[0], cut: null } : no;
    }
    const [, mods, kwRaw, name] = m;
    const kw = kwRaw.replace(/\s+/, " ");
    if (kw === "import" || kw === "package" || kw === "init" || /\boverride\b/.test(mods) || /\b(?:private|protected|internal)\b/.test(mods)) return no;
    if (/^(?:class|interface|object|companion object)$/.test(kw)) return { keep: true, kind: /\benum\b/.test(mods) ? "enum" : kw === "companion object" ? "companion" : kw, name, container: true };
    if (kw === "val" || kw === "var") return { keep: true, kind: kw, name, cut: /\bconst\b/.test(mods) ? null : "init" };
    if (kw === "fun") return { keep: true, kind: "fun", name, cut: block ? "block" : "expr" };
    if (kw === "constructor") return { keep: true, kind: "constructor", name: "", cut: block ? "block" : "delegate" };
    return { keep: true, kind: kw, name, cut: null };
  }
  // typescript
  let body = text;
  if (depth === 0) {
    if (/^export\s*\*/.test(body)) return { keep: true, kind: "reexport", name: "", raw: true };
    if (!/^export\b/.test(body) || /^export\s*\{\s*\}/.test(body)) return no;
    if (/^export\s*\{/.test(body)) return { keep: true, kind: "reexport", name: "", raw: true };
  }
  body = body.replace(/^export\s+(?:default\s+)?/, "");
  const m = /^((?:(?:declare|public|private|protected|static|readonly|abstract|override|async|get|set|accessor)\s+)*)(interface|type|class|namespace|enum|function|const|let|var|constructor)?\b\s*([\w$]*)/.exec(body);
  if (!m) return no;
  const [, mods, kw, name] = m;
  if (/\b(?:private|protected|override)\b/.test(mods) || /^#/.test(body)) return no;
  if (kw === "interface" || kw === "type" || kw === "enum") return { keep: true, kind: kw, name, raw: true };
  if (kw === "class" || kw === "namespace") return { keep: true, kind: kw, name, container: true };
  if (kw === "function") return { keep: true, kind: "function", name, cut: "block" };
  if (kw === "const" || kw === "let" || kw === "var") return { keep: true, kind: "const", name, cut: "init" };
  if (kw === "constructor") return { keep: true, kind: "constructor", name: "", cut: block ? "block" : null };
  // a class member: a method, an accessor or a field
  const first = /^[\w$]+/.exec(body.slice(mods.length));
  const mname = first ? first[0] : "";
  return { keep: true, kind: block ? "method" : "field", name: mname, cut: block ? "block" : "init" };
}

// ---------------------------------------------------------------------------------------------------------------
// Rendering

/** The text of a statement with its implementation cut off; `h` is the masked text and `raw` the source text of the same range. */
function cutHead(lang, c, h, raw) {
  const trimTo = (at) => raw.slice(0, at).replace(/\s+$/, "");
  const eq = equalsAt(h);
  if (c.cut === "init" && eq >= 0) {
    // Swift: a `var` loses its initializer, a `let` is a constant and keeps it. Kotlin and TypeScript: a property
    // with a declared type loses it; one without keeps it (the value is then the only thing that says what it is).
    const annotated = colonAt(h.slice(0, eq)) >= 0;
    if (lang === "swift" ? c.kind === "var" : annotated) return trimTo(eq);
  } else if (c.cut === "expr" && eq >= 0) { // `fun f(): T = expr`
    return trimTo(eq);
  } else if (c.cut === "delegate") { // `constructor(...) : this(...)`
    const open = h.indexOf("(");
    if (open >= 0) { const colon = h.indexOf(":", closer(h, open)); if (colon >= 0) return trimTo(colon); }
  }
  return raw;
}

/** Is there a blank line right before `at` in `src`, not looking before `from`? */
const blankBefore = (src, from, at) => /\n[ \t]*\n[ \t]*$/.test(src.slice(from, at));

/** Lines of the declarations in `src[start, end)`, at their original indentation. */
function render(src, m, start, end, lang, depth, parent, names) {
  const out = [];
  const push = (blank, ...lines) => { if (blank && out.length && out.at(-1) !== "") out.push(""); out.push(...lines); };
  const stmts = statements(m, start, end, lang);
  let carry = null; // the docs and attribute lines of a statement that is only an attribute
  for (const st of stmts) {
    const headMasked = m.slice(st.s, st.head);
    const hasBlock = st.blockStart >= 0;
    // `@MainActor @Observable` alone on its line belongs to the declaration below it.
    if (lang !== "ts" && !hasBlock && /^(?:@[\w.]+(?:\([^)]*\))?\s*)+$/.test(headMasked.trim())) {
      const d = docsIn(src, st.gap, st.s, lang);
      carry = carry ?? { docs: d.lines, first: d.first < 0 ? st.s : d.first, from: st.gap, attrs: [] };
      carry.attrs.push(src.slice(st.s, st.head).replace(/\s+$/, ""));
      continue;
    }
    const own = docsIn(src, st.gap, st.s, lang);
    const docs = carry ? carry.docs : own.lines;
    const leadAt = carry ? carry.first : own.first < 0 ? st.s : own.first;
    const from = carry ? carry.from : st.gap;
    const attrs = carry ? carry.attrs : [];
    carry = null;
    const c = classify(lang, (attrs.length ? attrs.join(" ") + " " : "") + headMasked, depth, parent, hasBlock);
    if (!c.keep) continue;
    const blank = blankBefore(src, from, leadAt);
    const indent = /^[ \t]*/.exec(src.slice(src.lastIndexOf("\n", st.s - 1) + 1, st.s))[0];
    const lead = [...docs, ...attrs.map((a) => indent + a)];
    const rawHead = src.slice(st.s, st.head).replace(/\s+$/, "");
    const verbatim = (text) => (indent + text.replace(/[ \t]+$/gm, "")).split("\n");
    const top = depth === 0 && c.name;

    if (c.raw) { // interfaces, type aliases, re-exports: everything, verbatim
      push(blank, ...lead, ...verbatim(src.slice(st.s, st.tail)));
      if (top) names.push(c.name);
      continue;
    }
    if (c.container) {
      const inner = hasBlock ? render(src, m, st.blockStart + 1, st.blockEnd, lang, depth + 1, c.kind, names) : [];
      const head = cutHead(lang, c, headMasked, rawHead);
      if (c.kind === "extension" && !inner.length && !/:/.test(headMasked)) continue; // an extension that adds nothing visible
      if (top) names.push(c.name);
      if (!hasBlock) push(blank, ...lead, ...verbatim(head));
      else if (!inner.length) push(blank, ...lead, ...verbatim(head + (m.slice(st.blockStart + 1, st.blockEnd).trim() ? " { \u2026 }" : " {}")));
      else push(blank, ...lead, ...verbatim(head + " {"), ...inner, indent + "}");
      continue;
    }
    // a signature, a property, a case
    let text;
    if (hasBlock && c.cut === "init" && colonAt(headMasked) < 0) text = src.slice(st.s, st.tail); // `const X = { ... } as const`: the value is the declaration
    else if (hasBlock) text = rawHead; // a body, or an initializer that is cut
    else text = cutHead(lang, c, headMasked, rawHead);
    text = text.replace(/\s*=$/, "");
    if (lang === "ts") text = text.replace(/;$/, "").replace(/^((?:(?:export|default|declare|public|static|abstract)\s+)*)async\s+/, "$1") + ";"; // `async` says nothing a `Promise<T>` return type does not
    push(blank, ...lead, ...verbatim(text));
    if (top && !(lang === "ts" && c.kind === "const" && colonAt(headMasked) >= 0)) names.push(c.name);
  }
  return out;
}

/** Folds runs of lines that differ only in digits. */
export function fold(lines) {
  const shape = (l) => l.replace(/\d+/g, "#");
  const out = [];
  for (let i = 0; i < lines.length;) {
    let j = i;
    const key = lines[i].trim() && !/^\s*(?:\/\/|\/\*|\*)/.test(lines[i]) ? shape(lines[i]) : null;
    while (key && j < lines.length && shape(lines[j]) === key) j++;
    const run = j - i;
    if (key && run >= FOLD_AT && /\d/.test(lines[i])) {
      const pad = /^\s*/.exec(lines[i])[0];
      out.push(lines[i], `${pad}// … ${run - 2} more with the same shape …`, lines[j - 1]);
      i = j;
    } else { out.push(lines[i]); i++; }
  }
  return out;
}

/**
 * The declarations of `source`, a file bindgen wrote for `lang` ("swift", "kotlin" or "ts").
 * Returns `{ text, names }`: the declarations as text, and the names of the top-level types and functions it declares.
 */
export function declarations(source, lang) {
  if (!LANGS.has(lang)) throw new Error(`declarations: unknown language "${lang}"`);
  const src = source.replace(/\r\n/g, "\n");
  const m = mask(src, lang);
  const names = [];
  let lines = render(src, m, 0, src.length, lang, 0, "file", names);
  lines = fold(lines);
  return { text: lines.join("\n") + (lines.length ? "\n" : ""), names: [...new Set(names)] };
}
