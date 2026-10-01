#!/usr/bin/env node
// Bundles what a hello-world web app ships of the JavaScript runtime, for scripts/wasm-size.sh
// (ADR-052). The entry is the app's own loader, `web/src/undra.ts` of an `undra init` project: it
// imports `@undra/runtime` and the generated bindings exactly as the app does. Vite (the one pinned
// by the TypeScript runtime's lockfile) builds it for production: tree-shaken, minified, the runtime
// and the bindings each in a chunk of their own so their sizes can be read apart. The worker script
// of the `wasm-worker` mode is a separate asset that only that mode loads.
//
//   node scripts/web-size-runtime.mjs <project-dir> <runtime-dir> <out-dir>
//
// Prints one JSON object: { runtime, bindings, app } with each chunk's path (relative to out-dir).
// Exit 3 when the runtime's node_modules are not installed (`npm ci` in <runtime-dir>).
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const [project, runtime, out] = process.argv.slice(2).map((p) => p && resolve(p));
if (!project || !runtime || !out) {
  console.error("usage: web-size-runtime.mjs <project-dir> <runtime-dir> <out-dir>");
  process.exit(2);
}
const viteEntry = join(runtime, "node_modules", "vite", "dist", "node", "index.js");
if (!existsSync(viteEntry)) {
  console.error(`web-size-runtime: ${viteEntry} is missing; run \`npm ci\` in ${runtime}`);
  process.exit(3);
}
const vite = await import(pathToFileURL(viteEntry).href);
const bindings = JSON.parse(readFileSync(join(project, "generated", "ts", "package.json"), "utf8")).name;

await vite.build({
  configFile: false,
  root: join(project, "web"),
  logLevel: "error",
  mode: "production",
  resolve: {
    alias: {
      [bindings]: join(project, "generated", "ts", "src", "index.ts"),
      "@undra/runtime": join(runtime, "src", "index.ts"),
    },
  },
  build: {
    outDir: out,
    emptyOutDir: true,
    assetsInlineLimit: 0,
    minify: true,
    rollupOptions: {
      input: join(project, "web", "src", "undra.ts"),
      preserveEntrySignatures: "exports-only",
      output: {
        codeSplitting: {
          groups: [
            { name: "undra-runtime", test: /runtimes[\\/]ts[\\/]@undra[\\/]runtime[\\/]/ },
            { name: "bindings", test: /generated[\\/]ts[\\/]/ },
          ],
        },
      },
    },
  },
});

const assets = readdirSync(join(out, "assets"));
/** The one emitted chunk whose name starts with `prefix` (and not with `not`, if given). */
const find = (prefix, not) => {
  const hits = assets.filter((f) => f.startsWith(prefix) && !(not && f.startsWith(not)) && f.endsWith(".js"));
  if (hits.length !== 1) throw new Error(`expected one ${prefix}*.js chunk, found ${JSON.stringify(hits)}`);
  return join("assets", hits[0]);
};
console.log(
  JSON.stringify({
    runtime: find("undra-runtime-"),
    bindings: find("bindings-"),
    app: find("undra-", "undra-runtime-"),
  }),
);
