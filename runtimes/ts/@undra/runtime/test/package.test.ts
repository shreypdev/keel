import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/*
 * The package's public entry points (docs/SPEC.md section 13): each subpath export points at a
 * module that exists, the framework adapters are optional peers, and the core imports none of them.
 */

const root = new URL("../", import.meta.url);
const pkg = JSON.parse(readFileSync(new URL("package.json", root), "utf8")) as {
  exports: Record<string, string | { types: string; default: string }>;
  peerDependencies: Record<string, string>;
  peerDependenciesMeta: Record<string, { optional: boolean }>;
  dependencies?: Record<string, string>;
};

describe("package exports", () => {
  it("lists the subpaths of SPEC section 13 that exist", () => {
    expect(Object.keys(pkg.exports).sort()).toEqual([".", "./package.json", "./react", "./solid", "./svelte", "./vue", "./wire", "./worker"]);
  });

  it("points every subpath at the compiled form of a source file that exists", () => {
    for (const [subpath, target] of Object.entries(pkg.exports)) {
      if (typeof target === "string") continue;
      const source = subpath === "." ? "src/index.ts" : subpath === "./wire" ? "src/wire/index.ts" : `src/${subpath.slice(2)}.ts`;
      expect(existsSync(new URL(source, root)), `${subpath}: ${source}`).toBe(true);
      const dist = source.replace(/^src\//, "./dist/").replace(/\.ts$/, "");
      expect(target, subpath).toEqual({ types: `${dist}.d.ts`, default: `${dist}.js` });
    }
  });

  it("declares the UI frameworks as optional peers and has no runtime dependency", () => {
    expect(Object.keys(pkg.peerDependencies).sort()).toEqual(["react", "solid-js", "svelte", "vue"]);
    for (const name of Object.keys(pkg.peerDependencies)) expect(pkg.peerDependenciesMeta[name]?.optional, name).toBe(true);
    expect(pkg.dependencies).toBeUndefined();
  });

  it("keeps every framework import inside its own adapter", () => {
    const adapters: Record<string, string> = { react: "src/react.ts", vue: "src/vue.ts", "solid-js": "src/solid.ts", svelte: "src/svelte.ts" };
    const sources = [
      "src/index.ts", "src/core.ts", "src/mirror.ts", "src/signal.ts", "src/object.ts", "src/stream.ts", "src/lifetime.ts", "src/worker.ts",
    ];
    for (const file of sources) {
      const text = readFileSync(new URL(file, root), "utf8");
      for (const framework of Object.keys(adapters)) {
        expect(text, `${file} must not import ${framework}`).not.toMatch(new RegExp(`from ["']${framework}(/[^"']*)?["']`));
      }
    }
    // The Svelte adapter takes a type from svelte and nothing else: no run-time import.
    expect(readFileSync(new URL("src/svelte.ts", root), "utf8")).toMatch(/^import type \{ Readable \} from "svelte\/store";$/m);
  });
});
