import { readFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/*
 * What a page loads up front is what `UndraCore` reaches by static imports (ADR-052: the hello page's JavaScript is gated at
 * 21,700 bytes gzipped, `scripts/web-size-runtime.mjs`). The modules below are code a hello page never runs, so the core
 * fetches each by a dynamic `import()` when it needs it; one static import of any of them, from the core or from anything the
 * core reaches, puts it in the first chunk, whole. This test is the cheap guard of that (the gate itself needs a build).
 */

const SRC = resolve(dirname(fileURLToPath(import.meta.url)), "../src");

/** The module specifiers `file` imports or re-exports at run time: a type-only import is erased and costs nothing. */
function staticImports(file: string): string[] {
  const text = readFileSync(file, "utf8");
  const specifiers: string[] = [];
  for (const match of text.matchAll(/^(import|export)(\s+type)?\s+([\s\S]*?)\s*from\s+["'](\.[^"']+)["']/gm)) {
    const [, , typeOnly, names, specifier] = match;
    if (typeOnly !== undefined) continue;
    // `import { type A, type B } from` is erased as a whole.
    const members = /^\{([\s\S]*)\}$/.exec((names ?? "").trim())?.[1]?.split(",").map((m) => m.trim()).filter((m) => m !== "");
    if (members !== undefined && members.length > 0 && members.every((m) => m.startsWith("type "))) continue;
    specifiers.push(specifier as string);
  }
  return specifiers;
}

/** Every source file reachable from `entry` by static imports, as a path relative to `src/`. */
function closure(entry: string): Set<string> {
  const seen = new Set<string>();
  const visit = (file: string): void => {
    const name = relative(SRC, file);
    if (seen.has(name)) return;
    seen.add(name);
    for (const specifier of staticImports(file)) visit(resolve(dirname(file), specifier.replace(/\.js$/, ".ts")));
  };
  visit(resolve(SRC, entry));
  return seen;
}

describe("what UndraCore loads up front", () => {
  const upFront = closure("core.ts");

  it("reaches the core's own modules (the test walks the right graph)", () => {
    for (const name of ["core.ts", "mirror.ts", "transport/wasm-main.ts", "adapters/default-ports.ts", "adapters/events.ts", "adapters/browser-events.ts", "panic.ts"]) {
      expect(upFront.has(name), name).toBe(true);
    }
  });

  it("does not reach what loads on demand: the ports, their codecs, the report builder of a trap, the background run, the other transports, recovery", () => {
    for (const name of [
      "adapters/ports.ts", // the Diagnostics and Timer ports of a native core, and every port builder
      "adapters/codecs.ts",
      "adapters/standard.ts", // the default ports' implementations
      "panic-report.ts", // the report of a trap, for an app with `onPanic` or `crashRecovery`
      "background.ts", // `runInBackground`, for a core with background work to drain (prod-ops review, ADR-052)
      "transport/remote.ts",
      "transport/wasm-worker.ts",
      "recovery.ts",
    ]) {
      expect(upFront.has(name), `${name} must be loaded by a dynamic import()`).toBe(false);
    }
  });
});
