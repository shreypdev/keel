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
// The runtime is what an installed app gets (ADR-057, decision 5): the package is built (`npm run build` of the runtime: the production
// flavour in `dist`, the readable one in `dist/dev`) and linked into the project's `node_modules`, and Vite resolves `@undra/runtime`
// through its `exports` with its default conditions in production mode, which selects the production flavour. A run whose chunk
// holds a sentence of the development flavour fails: the number would be of the wrong build.
//
//   node scripts/web-size-runtime.mjs <project-dir> <runtime-dir> <out-dir>
//   UNDRA_SIZE_MODULES=1 node scripts/web-size-runtime.mjs ...   also print, on stderr, the unminified
//                                                                bytes each source module contributes
//                                                                to each chunk (what grew: ADR-052, section 5);
//                                                                `=exports` adds the exports each one keeps
//   UNDRA_SIZE_TARGET=es2020 node scripts/web-size-runtime.mjs ...   build for that target instead of the pinned
//                                                                Vite's default (Vite 6's default lowers `#private`)
//   UNDRA_SIZE_SOURCEMAP=1 node scripts/web-size-runtime.mjs ... also write each chunk's source map next to it (hidden: the
//                                                                chunks are byte for byte the same), for
//                                                                scripts/web-size-attribute.mjs
//
// Prints one JSON object: { runtime, bindings, app, lazy } with each chunk's path (relative to out-dir);
// `lazy` lists the other JavaScript chunks (loaded on demand).
// Exit 3 when the runtime's node_modules are not installed (`npm ci` in <runtime-dir>).
import { execFileSync } from "node:child_process";
import { existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, rmSync, symlinkSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
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

// The package as an app installs it: built, then linked where both the app's loader and the generated bindings resolve a bare import.
if (!process.env.UNDRA_SIZE_NO_BUILD) execFileSync(process.execPath, [join(runtime, "scripts", "build.mjs")], { cwd: runtime, stdio: ["ignore", "ignore", "inherit"] });
const linked = join(project, "node_modules", "@undra", "runtime");
mkdirSync(dirname(linked), { recursive: true });
if (existsSync(linked) || lstatSync(linked, { throwIfNoEntry: false })) rmSync(linked, { recursive: true, force: true });
symlinkSync(runtime, linked, "dir");
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
        .map(([id, m]) => [m.renderedLength, id.replace(/^.*[\\/](?=runtimes[\\/]|generated[\\/]|web[\\/])/, ""), m.renderedExports ?? []])
        .sort((a, b) => b[0] - a[0]);
      for (const [length, id, exports] of rows) {
        if (length > 0) console.error(`  ${String(length).padStart(7)}  ${id}${process.env.UNDRA_SIZE_MODULES === "exports" ? `  [${exports.join(" ")}]` : ""}`);
      }
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
    },
  },
  build: {
    outDir: out,
    emptyOutDir: true,
    assetsInlineLimit: 0,
    minify: true,
    sourcemap: process.env.UNDRA_SIZE_SOURCEMAP ? "hidden" : false,
    // The pinned Vite's own default target (ES2022-capable browsers) unless asked: UNDRA_SIZE_TARGET=es2020 is
    // what an app on Vite 6's default ships (`#private` fields become WeakMap helpers).
    ...(process.env.UNDRA_SIZE_TARGET ? { target: process.env.UNDRA_SIZE_TARGET } : {}),
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

// The number must be of the production flavour: its chunk says no sentence of the development table (a build that resolved the other
// flavour, or a module that kept its prose, would measure something no production page ships).
const messages = readFileSync(join(runtime, "src", "messages.ts"), "utf8");
const sentences = [...messages.matchAll(/^ {2}\d+: "((?:[^"\\]|\\.)*)",/gm)].map((m) => JSON.parse(`"${m[1]}"`)).filter((t) => t.length >= 30 && !t.includes("{"));
const code = readFileSync(join(out, runtimeChunk), "utf8");
const leaked = sentences.find((t) => code.includes(t));
if (leaked !== undefined) throw new Error(`web-size-runtime: the runtime chunk holds a sentence of the development flavour (${JSON.stringify(leaked)}): the package did not resolve to its production build`);
console.log(JSON.stringify({ runtime: runtimeChunk, bindings: bindingsChunk, app: appChunk, lazy }));
