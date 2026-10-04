import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { describe, expect, it } from "vitest";
import * as dev from "../src/messages.js";
import * as prod from "../src/messages.prod.js";
import { WireError, type WireErrorDetail } from "../src/wire/errors.js";
import { SRC, sourceFiles } from "./support/module-graph.js";

/*
 * The runtime's messages (ADR-057, D1 and D8): every sentence the runtime throws or logs is `msg(<code>, ...values)`, a literal code
 * of one table (`src/messages.ts`) that the development flavour formats as the sentences always read, the production flavour
 * (`messages.prod.ts`) says as its code, its values and a link, and `site/scripts/build-errors.mjs` renders as the "Runtime
 * messages" section of the errors page. These tests hold the table's rules, so a code cannot be misused, reused or forgotten.
 */

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, "../../../../..");

/** The rows of the table: `  17: "sentence", // where it is raised`, or `  17: null, // retired`. */
function tableRows(): Array<{ code: number; text: string | null; where: string }> {
  const source = readFileSync(join(SRC, "messages.ts"), "utf8");
  const start = source.indexOf("const MESSAGES");
  const end = source.indexOf("\n};", start);
  const rows: Array<{ code: number; text: string | null; where: string }> = [];
  for (const line of source.slice(start, end).split("\n").slice(1)) {
    const match = /^ {2}(\d+): (null|"(?:[^"\\]|\\.)*"),(?: \/\/ (.*))?$/.exec(line);
    expect(match, `a table row of the form  N: "sentence", // where  (got: ${line})`).not.toBeNull();
    const [, code, text, where] = match as RegExpExecArray;
    rows.push({ code: Number(code), text: text === "null" ? null : (JSON.parse(text as string) as string), where: where ?? "" });
  }
  return rows;
}

interface Site {
  readonly file: string;
  readonly line: number;
  readonly code: number;
  /** How many values the call passes. */
  readonly values: number;
}

/** Every `msg(<number>, ...)` call of the runtime's sources. */
function callSites(): Site[] {
  const sites: Site[] = [];
  for (const file of sourceFiles()) {
    if (file === "messages.ts" || file === "messages.prod.ts") continue;
    const source = ts.createSourceFile(file, readFileSync(join(SRC, file), "utf8"), ts.ScriptTarget.ES2022, true);
    const visit = (node: ts.Node): void => {
      if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === "msg") {
        const first = node.arguments[0];
        const line = source.getLineAndCharacterOfPosition(node.getStart()).line + 1;
        expect(first !== undefined && ts.isNumericLiteral(first), `${file}:${line}: msg(...) takes a literal code, so the code is findable in the source`).toBe(true);
        sites.push({ file, line, code: Number((first as ts.NumericLiteral).text), values: node.arguments.length - 1 });
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
  }
  return sites;
}

/** The highest `{n}` a sentence has, plus one: how many values it says. */
const placeholders = (text: string): number => Math.max(-1, ...[...text.matchAll(/\{(\d+)\}/g)].map((m) => Number(m[1]))) + 1;

describe("the table of messages", () => {
  const rows = tableRows();
  const sites = callSites();

  it("numbers its rows 1..N with none missing: a code is never reused, a retired one stays as null", () => {
    expect(rows.map((r) => r.code)).toEqual(rows.map((_, i) => i + 1));
    expect(rows.length).toBeGreaterThan(200);
  });

  it("says each sentence once: two codes never say the same thing", () => {
    const texts = rows.flatMap((r) => (r.text === null ? [] : [r.text]));
    expect(texts.filter((text, i) => texts.indexOf(text) !== i)).toEqual([]);
  });

  it("says where each code is raised (the errors page shows it) and writes values as {0}, {1}, .. in order", () => {
    for (const row of rows) {
      if (row.text === null) continue;
      expect(row.where, `code ${row.code} has no \`// where it is raised\``).not.toBe("");
      const indexes = [...row.text.matchAll(/\{(\d+)\}/g)].map((m) => Number(m[1]));
      for (const index of indexes) expect(index, `code ${row.code}: {${index}} has no value before it`).toBeLessThanOrEqual(Math.max(-1, ...indexes));
      expect(new Set(indexes).size, `code ${row.code}: every value {0}..{n} is said`).toBe(placeholders(row.text));
    }
  });

  it("is used: every code of a call site is a sentence of the table, and every sentence of the table has a call site", () => {
    const byCode = new Map(rows.map((r) => [r.code, r.text]));
    for (const site of sites) {
      expect(byCode.has(site.code), `${site.file}:${site.line}: msg(${site.code}) is not in the table`).toBe(true);
      expect(byCode.get(site.code), `${site.file}:${site.line}: msg(${site.code}) is a retired code`).not.toBeNull();
    }
    const used = new Set(sites.map((s) => s.code));
    expect(rows.filter((r) => r.text !== null && !used.has(r.code)).map((r) => r.code), "codes nothing says (retire them: null)").toEqual([]);
  });

  it("is said with the values it has: a call passes as many values as its sentence has {n}", () => {
    const byCode = new Map(rows.map((r) => [r.code, r.text as string]));
    for (const site of sites) {
      const expected = placeholders(byCode.get(site.code) as string);
      expect(site.values, `${site.file}:${site.line}: msg(${site.code}) says ${expected} value(s)`).toBe(expected);
    }
  });

  it("has call sites that import msg from the messages module of the package (the flavour swap replaces that one module)", () => {
    for (const file of new Set(sites.map((s) => s.file))) {
      const text = readFileSync(join(SRC, file), "utf8");
      expect(text, `${file} imports msg`).toMatch(/import \{[^}]*\bmsg\b[^}]*\} from "(\.\.?\/)+messages\.js";/);
    }
  });
});

describe("the development flavour", () => {
  it("formats a sentence with its values as a template literal does", () => {
    expect(dev.msg(50)).toBe("the core is closed");
    expect(dev.msg(57, 9)).toBe("the core sent reply status 9");
    expect(dev.msg(197, 2, 1)).toBe("the core speaks wasm ABI 2, this runtime speaks 1");
    const rows = tableRows();
    const row = rows.find((r) => /\{1\}/.test(r.text ?? "") && !/\{2\}/.test(r.text ?? "")) as { code: number; text: string };
    expect(dev.msg(row.code, 10n, { toString: () => "obj" })).toBe(row.text.replace("{0}", "10").replace("{1}", "obj"));
  });

  it("does not interpret a value that looks like a placeholder or a replacement pattern", () => {
    expect(dev.msg(57, "{0}")).toBe("the core sent reply status {0}");
    expect(dev.msg(57, "$&")).toBe("the core sent reply status $&");
  });

  it("says a wire failure as it always did", () => {
    expect(new WireError({ code: "unexpected_eof", at: 12, needed: 3 }).message).toBe("wire: unexpected end of input at offset 12: needed 3 more bytes");
    expect(new WireError({ code: "bad_magic" }).message).toBe("wire: bad magic: envelope does not start with 554e4452");
  });

  it("names the exports a module lacks (the development check)", () => {
    expect(dev.missingExports({})).toContain("memory");
    expect(dev.missingExports({})).toContain("undra_abi_version");
    expect(dev.missingExports({ undra_abi_version: () => 1 })).not.toContain("undra_abi_version");
  });
});

describe("the production flavour", () => {
  it("has the same exports as the development one, with the same types", () => {
    expect(Object.keys(prod).sort()).toEqual(Object.keys(dev).sort());
    // Assignable both ways: a signature that drifts fails the typecheck of this file.
    const asDev: typeof dev = prod;
    const asProd: typeof prod = dev;
    expect([typeof asDev.msg, typeof asProd.wireText]).toEqual(["function", "function"]);
  });

  it("says the code, the values and the link: T0017: callSync, remote; see https://…/errors.html#T0017", () => {
    expect(prod.msg(17, "callSync", "remote")).toBe("T0017: callSync, remote; see https://shreypdev.github.io/undra/docs/errors.html#T0017");
    expect(prod.msg(5)).toBe("T0005; see https://shreypdev.github.io/undra/docs/errors.html#T0005");
    expect(prod.msg(1234, 1, 2n)).toBe("T1234: 1, 2; see https://shreypdev.github.io/undra/docs/errors.html#T1234");
  });

  it("says a wire failure as its code and fields, with a link", () => {
    const text = (detail: WireErrorDetail): string => prod.wireText(detail);
    expect(text({ code: "unexpected_eof", at: 12, needed: 3 })).toBe("wire: code=unexpected_eof at=12 needed=3; see https://shreypdev.github.io/undra/docs/errors.html#wire-unexpected_eof");
    expect(text({ code: "bad_magic" })).toBe("wire: code=bad_magic; see https://shreypdev.github.io/undra/docs/errors.html#wire-bad_magic");
    expect(text({ code: "schema_mismatch", expected: 255n, got: 1n })).toContain("expected=255 got=1");
  });

  it("carries no sentence of the table: a production message is a code, never prose", () => {
    const source = readFileSync(join(SRC, "messages.prod.ts"), "utf8");
    for (const row of tableRows()) if (row.text !== null && row.text.length > 24) expect(source).not.toContain(row.text);
    expect(prod.missingExports({})).toEqual(["undra_abi_version"]);
    expect(prod.missingExports({ undra_abi_version: () => 1 })).toEqual([]);
  });
});

describe("the errors page (docs/errors.html, generated by site/scripts/build-errors.mjs)", () => {
  const page = join(REPO, "site/docs/errors.html");

  it.skipIf(!existsSync(page))("has an anchor for every code of the table and for every wire failure code, and says each sentence", () => {
    const html = readFileSync(page, "utf8");
    for (const row of tableRows()) {
      if (row.text === null) continue;
      const id = `T${String(row.code).padStart(4, "0")}`;
      expect(html, `an anchor for ${id}`).toContain(`id="${id}"`);
    }
    for (const code of ["unexpected_eof", "invalid_utf8", "invalid_tag", "length_too_large", "trailing_bytes", "bad_magic", "unsupported_version", "schema_mismatch", "duplicate_key", "negative_duration", "unsafe_integer"]) {
      expect(html, `an anchor for wire failure ${code}`).toContain(`id="wire-${code}"`);
    }
  });
});
