import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/*
 * The built package (`npm run build`, ADR-057 D2): `dist` is the production flavour and `dist/dev` the readable one, and the
 * `exports` map picks between them by condition. These tests run only where `dist` exists (the CI job builds it first; `npm test`
 * on a fresh checkout skips them), in child processes so that they see what Node resolves and runs natively, not what Vitest
 * transforms.
 */

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DIST = join(ROOT, "dist");
const built = existsSync(join(DIST, "index.js")) && existsSync(join(DIST, "dev", "index.js"));

function files(dir: string, base = dir): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry);
    return statSync(path).isDirectory() ? files(path, base) : [relative(base, path)];
  });
}

/** Runs `script` as an ES module in a fresh Node whose working directory is the package (so `@undra/runtime` names itself). */
function node(script: string, ...flags: string[]): string {
  return execFileSync(process.execPath, [...flags, "--input-type=module", "-e", script], { cwd: ROOT, encoding: "utf8" }).trim();
}

describe.skipIf(!built)("the built package", () => {
  it("resolves the production build by default and the readable one for `development` and `react-native`", () => {
    const resolveRoot = `console.log(import.meta.resolve("@undra/runtime"))`;
    expect(node(resolveRoot)).toMatch(/\/dist\/index\.js$/);
    expect(node(resolveRoot, "--conditions=development")).toMatch(/\/dist\/dev\/index\.js$/);
    expect(node(resolveRoot, "--conditions=react-native")).toMatch(/\/dist\/dev\/index\.js$/);
    for (const subpath of ["wire", "react", "worker", "realtime", "db", "vite"]) {
      const specifier = `console.log(import.meta.resolve("@undra/runtime/${subpath}"))`;
      expect(node(specifier), subpath).toMatch(new RegExp(`/dist/${subpath === "wire" ? "wire/index" : subpath}\\.js$`));
      expect(node(specifier, "--conditions=development"), `${subpath} (development)`).toMatch(new RegExp(`/dist/dev/${subpath === "wire" ? "wire/index" : subpath}\\.js$`));
    }
  });

  it("has the same modules and the same declarations in both flavours: only messages.js is another file", () => {
    const prod = files(DIST).filter((f) => !f.startsWith("dev/"));
    const dev = files(join(DIST, "dev"));
    expect(prod.filter((f) => f.endsWith(".js")).sort()).toEqual(dev.filter((f) => f.endsWith(".js")).sort());
    expect(prod.filter((f) => f.endsWith(".d.ts")).sort()).toEqual(dev.filter((f) => f.endsWith(".d.ts")).sort());
    expect(prod.some((f) => f.includes("messages.prod")), "the swap leaves no messages.prod.* behind").toBe(false);
    expect(dev.some((f) => f.includes("messages.prod"))).toBe(false);
    for (const file of dev.filter((f) => f.endsWith(".d.ts"))) {
      expect(readFileSync(join(DIST, file), "utf8"), file).toBe(readFileSync(join(DIST, "dev", file), "utf8"));
    }
    expect(readFileSync(join(DIST, "messages.js"), "utf8")).not.toBe(readFileSync(join(DIST, "dev", "messages.js"), "utf8"));
  });

  it("points every source map at the sources of src/ (the production copy is one directory higher than the readable build)", () => {
    for (const dir of [DIST, join(DIST, "dev")]) {
      for (const file of ["core.js.map", "messages.js.map", "wire/reader.js.map", "transport/framed.d.ts.map"]) {
        const map = JSON.parse(readFileSync(join(dir, file), "utf8")) as { sources: string[] };
        const source = file === "messages.js.map" && dir === DIST ? "messages.prod.ts" : file.replace(/\.(js|d\.ts)\.map$/, ".ts");
        expect(map.sources, `${dir} ${file}`).toHaveLength(1);
        expect(map.sources[0], `${dir} ${file}`).toMatch(new RegExp(`${source.replace(".", "\\.")}$`));
        expect(existsSync(resolve(dir, dirname(file), map.sources[0] as string)), `${dir}/${file} -> ${map.sources[0]}`).toBe(true);
      }
    }
  });

  it("throws the same classes, kinds and fields in both and says a sentence only in the readable one", () => {
    const script = (path: string) => `
      const rt = await import(${JSON.stringify(path)});
      const out = [];
      const grab = (f) => { try { f(); } catch (e) { out.push({ name: e.constructor.name, kind: e.kind ?? null, code: e.code ?? null, message: e.message }); } };
      grab(() => rt.encodeUuid("nope"));
      grab(() => rt.decodeValue(rt.codecs.u32, new Uint8Array(1)));
      grab(() => { throw new rt.UndraCallError.Panicked("boom", ""); });
      grab(() => { throw new rt.UndraRestoreError(5); });
      console.log(JSON.stringify(out));
    `;
    const dev = JSON.parse(node(script(join(DIST, "dev", "index.js")))) as Array<{ name: string; kind: string | null; code: string | null; message: string }>;
    const prod = JSON.parse(node(script(join(DIST, "index.js")))) as Array<{ name: string; kind: string | null; code: string | null; message: string }>;
    expect(prod.map(({ message, ...rest }) => rest)).toEqual(dev.map(({ message, ...rest }) => rest));
    expect(dev.map((e) => e.name)).toEqual(["RangeError", "WireError", "Panicked", "UndraRestoreError"]);
    for (const [i, error] of prod.entries()) {
      expect(error.message, error.name).toMatch(/ — https:\/\/shreypdev\.github\.io\/undra\/docs\/errors\.html#(T\d{4}|wire-\w+)$/);
      expect((dev[i] as { message: string }).message, error.name).not.toMatch(/ — https:\/\//);
    }
  });

  it("exports the same names from every entry in both flavours", () => {
    const script = (dir: string) => `
      const out = {};
      for (const entry of ["index.js", "wire/index.js", "worker.js", "realtime.js", "db.js"]) {
        out[entry] = Object.keys(await import(${JSON.stringify(dir)} + "/" + entry)).sort();
      }
      console.log(JSON.stringify(out));
    `;
    const dev = JSON.parse(node(script(join(DIST, "dev")))) as Record<string, string[]>;
    const prod = JSON.parse(node(script(DIST))) as Record<string, string[]>;
    expect(prod).toEqual(dev);
    expect(dev["index.js"]?.length).toBeGreaterThan(100);
  });

  it("`codecs` is a module namespace natively: a member cannot be replaced or added, and it is not a frozen plain object (ADR-057 D9)", () => {
    const out = JSON.parse(
      node(`
        import { codecs } from ${JSON.stringify(join(DIST, "index.js"))};
        const attempt = (f) => { try { f(); return "no error"; } catch (e) { return e.constructor.name; } };
        console.log(JSON.stringify({
          replace: attempt(() => { "use strict"; codecs.u8 = codecs.u16; }),
          add: attempt(() => { "use strict"; codecs.extra = 1; }),
          remove: attempt(() => { "use strict"; delete codecs.u8; }),
          extensible: Object.isExtensible(codecs),
          frozen: Object.isFrozen(codecs),
          sealed: Object.isSealed(codecs),
          keys: Object.keys(codecs).length,
        }));
      `),
    ) as Record<string, unknown>;
    expect(out).toEqual({ replace: "TypeError", add: "TypeError", remove: "TypeError", extensible: false, frozen: false, sealed: true, keys: 24 });
  });
});
