#!/usr/bin/env node
// Bundles what a hello-world web app ships of the JavaScript runtime, for scripts/wasm-size.sh
// (ADR-052). The entry is the app's own loader, `web/src/undra.ts` of an `undra init` project: it
// imports `@undra/runtime` and the generated bindings exactly as the app does. Vite (the one pinned
// by the TypeScript runtime's lockfile) builds it for production: tree-shaken, minified, the runtime
// and the bindings each in a chunk of their own so their sizes can be read apart.
//
// What is measured is what the page loads **up front**: the runtime modules the entry reaches by static
// imports (Rolldown's `$initial` tag), in one chunk. A runtime module only a dynamic `import()` reaches (the
// `wasm-worker` and `remote` transports, which `UndraCore.load` fetches when the app asks for that mode) is a
// chunk of its own, loaded on demand, and is reported next to the number (`lazy`), not in it (ADR-052,
// amendment of ts-size-e4). The `wasm-worker` mode's Worker script is likewise a separate asset.
//
//   node scripts/web-size-runtime.mjs <project-dir> <runtime-dir> <out-dir>
//   UNDRA_SIZE_MODULES=1 node scripts/web-size-runtime.mjs ...   also print, on stderr, the unminified
//                                                                bytes each source module contributes
//                                                                to each chunk (what grew: ADR-052, section 5)
//
// Prints one JSON object: { runtime, bindings, app, lazy } with each chunk's path (relative to out-dir);
// `lazy` lists the other JavaScript chunks (loaded on demand).
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

/** Collects each module's rendered (unminified) size per chunk, for `UNDRA_SIZE_MODULES=1`. */
const moduleReport = {
  name: "undra-size-modules",
  generateBundle(_options, bundle) {
    if (!process.env.UNDRA_SIZE_MODULES) return;
    for (const chunk of Object.values(bundle)) {
      if (chunk.type !== "chunk") continue;
      console.error(`chunk ${chunk.fileName} (${chunk.code.length} bytes minified)`);
      const rows = Object.entries(chunk.modules)
        .map(([id, m]) => [m.renderedLength, id.replace(/^.*[\\/](?=runtimes[\\/]|generated[\\/]|web[\\/])/, "")])
        .sort((a, b) => b[0] - a[0]);
      for (const [length, id] of rows) if (length > 0) console.error(`  ${String(length).padStart(7)}  ${id}`);
    }
  },
};

await vite.build({
  configFile: false,
  plugins: [moduleReport],
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
            // `$initial`: only the modules the page loads up front; the ones behind a dynamic import() stay in chunks of their own.
            { name: "undra-runtime", test: /runtimes[\\/]ts[\\/]@undra[\\/]runtime[\\/]/, tags: ["$initial"] },
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
const runtimeChunk = find("undra-runtime-");
const bindingsChunk = find("bindings-");
const appChunk = find("undra-", "undra-runtime-");
const lazy = assets
  .filter((f) => f.endsWith(".js"))
  .map((f) => join("assets", f))
  .filter((f) => ![runtimeChunk, bindingsChunk, appChunk].includes(f));
console.log(JSON.stringify({ runtime: runtimeChunk, bindings: bindingsChunk, app: appChunk, lazy }));
