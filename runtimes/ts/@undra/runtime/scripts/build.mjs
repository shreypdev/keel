#!/usr/bin/env node
// Builds @undra/runtime's `dist` (ADR-057): the same sources compiled twice over, as two flavours.
//
//   dist/dev   the development build: what `tsc` writes, the sentences of src/messages.ts and readable private names. It is what the
//              `development` and `react-native` export conditions resolve (Vite's dev server, Vitest, webpack in development mode,
//              Metro).
//   dist       the production build: the same files with `messages.js` replaced by src/messages.prod.ts (a message says its code, its
//              values and a link instead of a sentence) and, once the rename pass runs, private properties shortened. It is what the
//              `default` condition resolves (a production build of an app, esbuild, Rollup, plain Node), and where the `types`
//              condition points: the declarations of the two flavours are the same.
//
// Source maps are published for both, each pointing at src/.
//
//   node scripts/build.mjs            (npm run build)
import { copyFileSync, cpSync, existsSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DIST = join(ROOT, "dist");
const DEV = join(DIST, "dev");
const tsc = join(ROOT, "node_modules", "typescript", "bin", "tsc");

const compile = (project) => execFileSync(process.execPath, [tsc, "-p", join(ROOT, project), "--outDir", DEV], { cwd: ROOT, stdio: "inherit" });

/** Every file under `dir`, relative to it. */
function walk(dir, base = dir) {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry);
    return statSync(path).isDirectory() ? walk(path, base) : [path.slice(base.length + 1)];
  });
}

// 1. The development build, and the Vite plugin (which only needs Node's types, so it has a project of its own).
rmSync(DIST, { recursive: true, force: true });
compile("tsconfig.build.json");
compile("tsconfig.vite.json");

// 2. The production build starts as a copy of it. The copy sits one directory higher, so the source maps' `../../src/..` becomes `../src/..`.
cpSync(DEV, DIST, { recursive: true });
for (const file of walk(DEV)) {
  if (!file.endsWith(".map")) continue;
  const path = join(DIST, file);
  const map = JSON.parse(readFileSync(path, "utf8"));
  if (Array.isArray(map.sources)) map.sources = map.sources.map((source) => source.replace(/^\.\.\//, ""));
  writeFileSync(path, JSON.stringify(map));
}

// 3. The flavours differ in one module: the production build's messages.js is messages.prod.js (and says so to debuggers).
const prod = join(DIST, "messages.prod.js");
if (!existsSync(prod)) throw new Error("build: messages.prod.js was not compiled; src/messages.prod.ts is missing from tsconfig.build.json");
writeFileSync(join(DIST, "messages.js"), readFileSync(prod, "utf8").replace("messages.prod.js.map", "messages.js.map"));
const prodMap = JSON.parse(readFileSync(join(DIST, "messages.prod.js.map"), "utf8"));
prodMap.file = "messages.js";
writeFileSync(join(DIST, "messages.js.map"), JSON.stringify(prodMap));
for (const dir of [DIST, DEV]) for (const extra of ["messages.prod.js", "messages.prod.js.map", "messages.prod.d.ts", "messages.prod.d.ts.map"]) rmSync(join(dir, extra), { force: true });
// The development build keeps its own messages.js; both flavours share one declaration of it (the two modules have the same exports and types).
copyFileSync(join(DEV, "messages.d.ts"), join(DIST, "messages.d.ts"));

console.log(`build: dist (production) and dist/dev (development) written, ${walk(DIST).filter((f) => f.endsWith(".js") && !f.startsWith("dev")).length} modules each`);
