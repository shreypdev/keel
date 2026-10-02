import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { decode } from "@jridgewell/sourcemap-codec";
import ts from "typescript";
import { afterEach, describe, expect, it } from "vitest";
// @ts-expect-error -- a build script (plain ESM), not part of the package's types
import { RESERVED, mangleDist, planRename, renameSource, propertyOccurrences } from "../scripts/mangle.mjs";

/*
 * The production build's rename pass (ADR-057, D2; `scripts/mangle.mjs`): private properties become `_a`, `_b`, .. through one cache,
 * except the four names generated code uses and the names that belong to someone else; nothing else about a module changes. These tests
 * run it on small modules for each rule and on the built package for what must hold of the whole.
 */

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DIST = join(ROOT, "dist");

const scratch: string[] = [];
afterEach(() => {
  for (const dir of scratch.splice(0)) rmSync(dir, { recursive: true, force: true });
});

/** A directory of modules, as `{ "a.js": "text" }`. */
function modules(files: Record<string, string>): string {
  const dir = mkdtempSync(join(tmpdir(), "undra-mangle-"));
  scratch.push(dir);
  for (const [name, text] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, name)), { recursive: true });
    writeFileSync(join(dir, name), text);
  }
  return dir;
}

const read = (dir: string, name: string): string => readFileSync(join(dir, name), "utf8");

describe("what is renamed", () => {
  it("renames a member the package declares, at every place it says it, through one cache for all the modules", () => {
    const dir = modules({
      "a.js": "export class A {\n  _pending = 0;\n  _flush() { return this._pending + this._other._pending; }\n}\n",
      "b.js": "import { A } from './a.js';\nexport function f(a) { a._pending = 1; return a._flush(); }\nexport const o = { _other: new A() };\n",
    });
    const result = mangleDist(dir);
    const a = result.cache["_pending"];
    const flush = result.cache["_flush"];
    const other = result.cache["_other"];
    expect([a, flush, other].every((n) => /^_[A-Za-z]{1,2}$/.test(n))).toBe(true);
    expect(new Set([a, flush, other]).size).toBe(3);
    expect(read(dir, "a.js")).toBe(`export class A {\n  ${a} = 0;\n  ${flush}() { return this.${a} + this.${other}.${a}; }\n}\n`);
    expect(read(dir, "b.js")).toContain(`a.${a} = 1; return a.${flush}();`);
    expect(read(dir, "b.js")).toContain(`{ ${other}: new A() }`);
    expect(JSON.parse(read(dir, "mangle-cache.json"))).toEqual(result.cache);
  });

  it("gives the shortest names to the names said most often", () => {
    const dir = modules({ "a.js": "class A { _rare = 1; _often = 2; m() { return this._often + this._often + this._often + this._rare; } }\n" });
    const { cache } = mangleDist(dir);
    expect(cache["_often"]).toBe("_a");
    expect(cache["_rare"]).toBe("_b");
  });

  it("keeps the four names generated code uses (SPEC 17.1), and says them as they are", () => {
    expect(RESERVED).toEqual(["_set", "_signals", "_apply", "_observeAll"]);
    const dir = modules({ "a.js": "export class S { _signals = []; _set(v) { this._v = v; } _apply() {} async _observeAll() {} }\n" });
    const { cache } = mangleDist(dir);
    expect(Object.keys(cache)).toEqual(["_v"]);
    expect(read(dir, "a.js")).toContain("_signals = []; _set(v) { this._a = v; } _apply() {} async _observeAll() {}");
  });

  it("keeps a name only ever read: it is another object's (a WebAssembly export, a library's method)", () => {
    const dir = modules({ "a.js": "export function boot(e) { e._initialize?.(); const { _sqlite3_malloc } = e; return e._sqlite3_malloc(1) + _sqlite3_malloc; }\nexport class A { _mine = 1; }\n" });
    const result = mangleDist(dir);
    expect(Object.keys(result.cache)).toEqual(["_mine"]);
    expect(result.foreign).toEqual(["_initialize", "_sqlite3_malloc"]);
    expect(read(dir, "a.js")).toContain("e._initialize?.(); const { _sqlite3_malloc } = e; return e._sqlite3_malloc(1)");
  });

  it("does not touch strings that only contain a name, comments, `constructor`, or names with two underscores", () => {
    const text = [
      "export class A {",
      "  constructor() { this._x = 1; this.__proto__ = null; }",
      "  toString() { return 'a._x, \"_x\"'; } // _x",
      "}",
      "export const o = { __dunder: 2, _: 3, _1: 4 };",
      "",
    ].join("\n");
    const dir = modules({ "a.js": text });
    const { cache } = mangleDist(dir);
    expect(cache).toEqual({ _x: "_a" });
    expect(read(dir, "a.js")).toBe(text.replace("this._x = 1", "this._a = 1"));
  });

  it("fails the build on a string that is exactly a renamed name: it would keep meaning the old one", () => {
    expect(() => mangleDist(modules({ "a.js": "export class A { _x = 1; get(o) { return o['_x']; } }\n" }))).toThrow(/a\.js says "_x" as a string/);
    expect(() => mangleDist(modules({ "a.js": "export class A { _x = 1; }\nexport const o = { '_x': 1 };\n" }))).toThrow(/says "_x" as a string/);
    // A string equal to a name that is not renamed (reserved, someone else's) is fine.
    expect(() => mangleDist(modules({ "a.js": "export class A { _set = 1; _x = 2; has(o) { return '_set' in o && '_initialize' in o; } }\n" }))).not.toThrow();
  });

  it("expands a shorthand property, which is also a variable: `{ _x }` becomes `{ _a: _x }`, with a default too", () => {
    const dir = modules({ "a.js": "export class A { _x = 1; }\nexport function f(_x, o) { const { _x: y, _x: z = 3 } = o; const { _x: w = 2 } = o; return { _x, y, z, w }; }\nexport function g({ _x = 5 }) { return _x; }\n" });
    mangleDist(dir);
    expect(read(dir, "a.js")).toContain("return { _a: _x, y, z, w }");
    expect(read(dir, "a.js")).toContain("const { _a: y, _a: z = 3 } = o; const { _a: w = 2 } = o;");
    expect(read(dir, "a.js")).toContain("function g({ _a: _x = 5 }) { return _x; }");
  });

  it("never gives a name that is already somewhere as a property, and never one a generated class declares", () => {
    const names = Array.from({ length: 60 }, (_, i) => `_n${i}`);
    const dir = modules({ "a.js": `export class A { ${names.map((n) => `${n} = 1;`).join(" ")} _b = 2; _resync() { return e._c + e._d; } }\n` });
    const { cache } = mangleDist(dir);
    const outputs = Object.values(cache) as string[];
    expect(new Set(outputs).size, "one output name each").toBe(outputs.length);
    // `_b` is declared (and so renamed itself), `_c` and `_d` are read and kept, `_resync` is a generated class's own method: none is an output.
    for (const taken of ["_c", "_d", "_resync"]) expect(outputs).not.toContain(taken);
  });

  it("fails the build when a private property is also an export's name (a rename cannot reach an export)", () => {
    const dir = modules({ "a.js": "export class A { _x = 1; }\nexport const _x = 2;\n" });
    expect(() => mangleDist(dir)).toThrow(/_x is both a private property and the name of an export/);
    const viaSpecifier = modules({ "a.js": "export class A { _y = 1; }\nconst k = 1;\nexport { k as _y };\n" });
    expect(() => mangleDist(viaSpecifier)).toThrow(/_y is both/);
  });

  it("skips the files it is told to (the Vite plugin is Node tooling, the development build is its own)", () => {
    const text = "export class A { _x = 1; }\n";
    const dir = modules({ "a.js": text, "vite.js": text, "dev/a.js": text });
    mangleDist(dir, { skip: (rel: string) => rel === "dev" || rel === "vite.js" });
    expect(read(dir, "vite.js")).toBe(text);
    expect(read(dir, "dev/a.js")).toBe(text);
    expect(read(dir, "a.js")).not.toBe(text);
  });

  it("is deterministic: the same modules give the same cache and the same text", () => {
    const text = { "a.js": "export class A { _a1 = 1; _b1 = 2; _c1 = 3; m() { return this._b1 + this._c1 + this._a1 + this._b1; } }\n", "b.js": "export const o = { _c1: 1, _z: 2 };\n" };
    const one = modules(text);
    const two = modules(text);
    expect(mangleDist(one).cache).toEqual(mangleDist(two).cache);
    expect(read(one, "a.js")).toBe(read(two, "a.js"));
    expect(read(one, "mangle-cache.json")).toBe(read(two, "mangle-cache.json"));
  });
});

describe("what is kept", () => {
  it("changes nothing but the names: comments (the tree-shaking annotations among them), blank lines and line structure stay", () => {
    const source = [
      "/** Doc. */",
      "export class A {",
      "  _count = 0; // a comment",
      "",
      "  bump() {",
      "    return this._count++;",
      "  }",
      "}",
      "export const made = /* @__PURE__ */ (() => new A())();",
      "",
    ].join("\n");
    const dir = modules({ "a.js": source });
    const { cache } = mangleDist(dir);
    const out = read(dir, "a.js");
    expect(out.replaceAll(cache["_count"], "_count")).toBe(source);
    expect(out).toContain("/* @__PURE__ */");
    expect(out.split("\n")).toHaveLength(source.split("\n").length);
  });

  it("moves a source map's columns with the edits, so that every mapped token still starts where its source token does", () => {
    const source = [
      "export class Queue {",
      "  private _pendingCalls = 0;",
      "  private _flushNow(limit: number): number { return this._pendingCalls + limit + this._pendingCalls; }",
      "  run(): number { const left = this._flushNow(2); return left + this._pendingCalls; }",
      "}",
      "",
    ].join("\n");
    const emitted = ts.transpileModule(source, { fileName: "queue.ts", compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, sourceMap: true, inlineSources: false } });
    const dir = modules({ "queue.js": emitted.outputText.replace(/\/\/# sourceMappingURL=.*$/m, ""), "queue.js.map": emitted.sourceMapText ?? "" });
    const before = read(dir, "queue.js");
    const mapBefore = JSON.parse(read(dir, "queue.js.map")) as { mappings: string };
    const { cache } = mangleDist(dir);
    const after = read(dir, "queue.js");
    const mapAfter = JSON.parse(read(dir, "queue.js.map")) as { mappings: string };
    expect(after).not.toBe(before);
    const token = (text: string, line: number, column: number): string => /^[\w$]*/.exec((text.split("\n")[line] ?? "").slice(column))?.[0] ?? "";
    const was = decode(mapBefore.mappings);
    const now = decode(mapAfter.mappings);
    let checked = 0;
    was.forEach((segments, line) => {
      segments.forEach((segment, index) => {
        const moved = (now[line] as typeof segments)[index] as typeof segment;
        const original = token(before, line, segment[0]);
        const expected = Object.hasOwn(cache, original) ? (cache[original] as string) : original;
        expect(token(after, line, moved[0]), `line ${line + 1}, column ${segment[0]}: ${original}`).toBe(expected);
        expect(moved.slice(1), "the source position is not touched").toEqual(segment.slice(1));
        checked++;
      });
    });
    expect(checked).toBeGreaterThan(20);
    expect(Object.keys(cache).sort()).toEqual(["_flushNow", "_pendingCalls"]);
  });

  it("parses what it reads: a module that does not parse is a build error, not a silent skip", () => {
    const dir = modules({ "a.js": "export class { \n" });
    expect(() => planRename(dir)).toThrow(/does not parse/);
    expect(() => propertyOccurrences("x.js", "let = ;")).toThrow(/does not parse/);
    expect(renameSource("a._x", [{ name: "_x", start: 2, end: 4, role: "read", shorthand: false }], { _x: "_a" }).code).toBe("a._a");
  });
});

/** Every `.js` of the built production flavour (outside `dev/`). */
function productionModules(dir = DIST, base = DIST): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) return entry === "dev" ? [] : productionModules(path, base);
    return path.endsWith(".js") ? [path] : [];
  });
}

describe.skipIf(!existsSync(join(DIST, "mangle-cache.json")))("the built package", () => {
  const cache = (): Record<string, string> => JSON.parse(readFileSync(join(DIST, "mangle-cache.json"), "utf8")) as Record<string, string>;

  it("renamed what the runtime declares, in the production flavour only", () => {
    const names = Object.keys(cache());
    expect(names.length).toBeGreaterThan(100);
    for (const name of ["_pending", "_transport", "_giveBack", "_era", "_undraClosed"]) expect(names, name).toContain(name);
    const dev = readFileSync(join(DIST, "dev", "core.js"), "utf8");
    const prod = readFileSync(join(DIST, "core.js"), "utf8");
    expect(dev).toContain("_pending");
    expect(prod).not.toContain("_pending");
    expect(prod).toContain(`this.${cache()["_pending"]}`);
  });

  it("left the contract with generated code, and what belongs to other objects, as it was", () => {
    const names = Object.keys(cache());
    for (const kept of [...RESERVED, "_initialize", "_getSqliteFree", "_sqlite3_malloc", "_sqlite3_bind_text", "_sqlite3_bind_blob"]) expect(names, kept).not.toContain(kept);
    expect(readFileSync(join(DIST, "object.js"), "utf8")).toMatch(/_signals/);
    expect(readFileSync(join(DIST, "signal.js"), "utf8")).toMatch(/_set\(/);
    expect(readFileSync(join(DIST, "transport", "wasm-main.js"), "utf8")).toContain("_initialize");
    expect(readFileSync(join(DIST, "db", "wa-sqlite-engine.js"), "utf8")).toContain("_sqlite3_malloc");
  });

  it("declares no private property in the production flavour but the renamed ones and the reserved four", () => {
    const left = new Set<string>();
    for (const file of productionModules()) {
      if (file.endsWith("/vite.js")) continue;
      for (const { name, role } of propertyOccurrences(file, readFileSync(file, "utf8")).found as Array<{ name: string; role: string }>) {
        if (role === "declared" && /^_[A-Za-z$]/.test(name) && !name.startsWith("__")) left.add(name);
      }
    }
    const allowed = new Set<string>([...RESERVED, ...Object.values(cache())]);
    expect([...left].filter((name) => !allowed.has(name))).toEqual([]);
    expect([...left].filter((name) => Object.hasOwn(cache(), name))).toEqual([]);
  });

  it("every renamed name is one letter or two after an underscore, and no two share one", () => {
    const outputs = Object.values(cache());
    for (const name of outputs) expect(name).toMatch(/^_[A-Za-z]{1,2}$/);
    expect(new Set(outputs).size).toBe(outputs.length);
  });

  it("keeps the readable build untouched", () => {
    expect(readFileSync(join(DIST, "dev", "core.js"), "utf8")).toContain("_pending");
  });

  it.skipIf(process.env.UNDRA_TEST_DIST === undefined)("is what the suite runs against under `npm run test:dist`: the core the tests import has the renamed members", async () => {
    const { UndraCore } = await import("../src/core.js");
    const names = Object.getOwnPropertyNames(UndraCore.prototype);
    expect(names).not.toContain("_giveBack");
    expect(names).toContain(cache()["_giveBack"]);
  });
});
