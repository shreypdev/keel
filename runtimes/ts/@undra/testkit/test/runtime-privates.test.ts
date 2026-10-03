import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/*
 * `@undra/runtime`'s production build renames every private property (a name that starts with an underscore, SPEC 17.1; ADR-057), except the
 * four that generated code uses. This package ships beside it and names none of the others on a runtime object: the sources below must not
 * read or write `x._name`. (A new need for one is a public member of the runtime, not an underscore name.)
 */

const SRC = join(dirname(fileURLToPath(import.meta.url)), "../src");
const ALLOWED = new Set(["_set", "_signals", "_apply", "_observeAll"]);

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry);
    return statSync(path).isDirectory() ? sources(path) : /\.(ts|tsx)$/.test(path) && !path.endsWith(".d.ts") ? [path] : [];
  });
}

describe("this package names no private member of the runtime", () => {
  it("reads and writes no `_name` property outside the four of SPEC 17.1", () => {
    const found: string[] = [];
    for (const file of sources(SRC)) {
      readFileSync(file, "utf8")
        .split("\n")
        .forEach((line, index) => {
          const code = line.replace(/\/\/.*$/, "").replace(/\/\*.*?\*\//g, "");
          for (const m of code.matchAll(/(?:\.|\?\.|\[\s*["'`])(_[A-Za-z]\w*)/g)) {
            if (!ALLOWED.has(m[1] as string)) found.push(`${file.slice(SRC.length + 1)}:${index + 1}: ${m[1]}`);
          }
        });
    }
    expect(found).toEqual([]);
  });
});
