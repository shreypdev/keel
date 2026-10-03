#!/usr/bin/env node
// Bundles what a hello-world web app ships of the JavaScript runtime, for scripts/wasm-size.sh
// (ADR-052, ADR-057). The entry is the app's own loader, `web/src/undra.ts` of an `undra init` project: it
// imports `@undra/runtime` and the generated bindings exactly as the app does. Vite (the one pinned
// by the TypeScript runtime's lockfile) builds it for production: tree-shaken, minified, the runtime
// and the bindings each in a chunk of their own so their sizes can be read apart.
//
// What is measured is what the page loads **up front**: the runtime modules the entry reaches by static
// imports (Rolldown's `$initial` tag), in one chunk. A runtime module only a dynamic `import()` reaches (the
// `wasm-worker` and `remote` transports, which `UndraCore.load` fetches when the app asks for that mode, the stream
// support, the standard ports, ..) is a chunk of its own, loaded on demand, and is reported next to the number
// (`lazy`), not in it (ADR-052, amendment of ts-size-e4). The `wasm-worker` mode's Worker script is likewise a
// separate asset. Vite's preload helper, the virtual module the build adds to whichever chunk has an `import()`,
// is not the runtime's code (an app with one `import()` of its own has it anyway, and a Vite upgrade that changes it
// should not fail a runtime gate): it is a chunk of its own, reported as `bundler` (ADR-057, D6). The same page is built
// once more with the helper left where Vite puts it, in the first chunk (`inside`), for the row that holds the other reading
// of the number.
//
// The runtime is what an installed app gets (ADR-057, decision 5): the package is built (`npm run build` of the runtime: the production
// flavour in `dist`, the readable one in `dist/dev`) and linked into the project's `node_modules`, and Vite resolves `@undra/runtime`
// through its `exports` with its default conditions in production mode, which selects the production flavour. A run whose chunk
// holds a sentence of the development flavour fails: the number would be of the wrong build.
//
// A third build is the page that uses everything (scripts/web-size-all-features.ts: the playground's bindings with crash recovery, a panic
// handler, `stats`, `snapshot`, `restore` and a background run, in `wasm-main` mode): its first chunk plus every on-demand
// chunk of the runtime but the Worker script is the row `web/all-features-runtime-js`. The plan may move bytes out of the first chunk; it
// may not make the page that uses every feature load more.
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
//   UNDRA_SIZE_EXTRA=0 node scripts/web-size-runtime.mjs ...     only the hello page with the helper beside the number (the attribution tool's)
//
// Prints one JSON object: `{ runtime, bindings, app, lazy, bundler, modules, inside, allFeatures }`: each chunk's path (relative to out-dir;
// `lazy` lists the other JavaScript chunks, loaded on demand; `modules` the runtime modules, relative to the package, that hold code in
// the first chunk; `inside` the first chunk of the build with the helper in it; `allFeatures` `{ runtime, lazy }`, its first chunk and its
// on-demand chunks without the Worker script).
// Exit 3 when the runtime's node_modules are not installed (`npm ci` in <runtime-dir>).
import { execFileSync } from "node:child_process";
import { copyFileSync, cpSync, existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, realpathSync, rmSync, symlinkSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

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

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "..");
/** The package's directory with symlinks resolved, which is how the bundler names its modules. */
const runtimeReal = realpathSync(runtime);

/** What each build reports about itself, through `generateBundle`. */
function reporter(label, into) {
  return {
    name: "undra-size-report",
    generateBundle(_options, bundle) {
      for (const file of Object.values(bundle)) {
        if (file.type === "asset" && file.fileName.endsWith(".js")) (into.workers ??= []).push(file.fileName);
        if (file.type !== "chunk") continue;
        // The runtime modules that hold code in the first chunk (a module of no rendered bytes is tree-shaken away: it is not in the page).
        if (file.name === "undra-runtime") {
          into.modules = Object.entries(file.modules)
            .filter(([id, m]) => m.renderedLength > 0 && id.startsWith(`${runtimeReal}/`))
            .map(([id]) => relative(runtimeReal, id))
            .sort();
        }
        if (!process.env.UNDRA_SIZE_MODULES || label !== "hello") continue;
        console.error(`chunk ${file.fileName} (${file.code.length} bytes minified)`);
        const rows = Object.entries(file.modules)
          .map(([id, m]) => [m.renderedLength, id.replace(/^.*[\\/](?=runtimes[\\/]|generated[\\/]|web[\\/])/, ""), m.renderedExports ?? []])
          .sort((a, b) => b[0] - a[0]);
        for (const [length, id, exports] of rows) {
          if (length > 0) console.error(`  ${String(length).padStart(7)}  ${id}${process.env.UNDRA_SIZE_MODULES === "exports" ? `  [${exports.join(" ")}]` : ""}`);
        }
      }
    },
  };
}

/** One Vite production build: `helper` is "beside" (Vite's preload helper in a chunk of its own) or "inside" (where Vite puts it). */
async function build({ label, input, outDir, helper, alias = {} }) {
  const report = {};
  await vite.build({
    configFile: false,
    plugins: [reporter(label, report)],
    root: join(project, "web"),
    logLevel: "error",
    mode: "production",
    resolve: { alias },
    build: {
      outDir,
      emptyOutDir: true,
      assetsInlineLimit: 0,
      minify: true,
      sourcemap: process.env.UNDRA_SIZE_SOURCEMAP ? "hidden" : false,
      // The pinned Vite's own default target (ES2022-capable browsers) unless asked: UNDRA_SIZE_TARGET=es2020 is
      // what an app on Vite 6's default ships (`#private` fields become WeakMap helpers).
      ...(process.env.UNDRA_SIZE_TARGET ? { target: process.env.UNDRA_SIZE_TARGET } : {}),
      rollupOptions: {
        input,
        preserveEntrySignatures: "exports-only",
        output: {
          codeSplitting: {
            groups: [
              // `$initial`: only the modules the page loads up front; the ones behind a dynamic import() stay in chunks of their own.
              { name: "undra-runtime", test: (id) => id.startsWith(`${runtimeReal}/`), tags: ["$initial"] },
              { name: "bindings", test: /generated[\\/]ts[\\/]|all-features[\\/]bindings[\\/]/ },
              ...(helper === "beside" ? [{ name: "bundler", test: /vite[\\/]preload-helper/, priority: 10 }] : []),
            ],
          },
        },
      },
    },
  });
  const assets = readdirSync(join(outDir, "assets")).filter((f) => f.endsWith(".js"));
  /** The one emitted chunk whose name starts with `prefix` (and not with `not`, if given). */
  const find = (prefix, not) => {
    const hits = assets.filter((f) => f.startsWith(prefix) && !(not && f.startsWith(not)));
    if (hits.length !== 1) throw new Error(`expected one ${prefix}*.js chunk, found ${JSON.stringify(hits)}`);
    return hits[0];
  };
  return { assets, find, report };
}

/** `file` of `dir` as a path relative to the first build's directory, which is what the caller resolves. */
const fromOut = (dir, file) => (dir === out ? join("assets", file) : join(relative(out, dir), "assets", file));

// ----- the hello page, the helper beside the number --------------------------------------------------------------
const helloEntry = join(project, "web", "src", "undra.ts");
const helloAlias = { [bindings]: join(project, "generated", "ts", "src", "index.ts") };
const hello = await build({ label: "hello", input: helloEntry, outDir: out, helper: "beside", alias: helloAlias });
const runtimeChunk = hello.find("undra-runtime-");
const bindingsChunk = hello.find("bindings-");
const appChunk = hello.find("undra-", "undra-runtime-");
const bundlerChunk = hello.find("bundler-");
const lazy = hello.assets.filter((f) => ![runtimeChunk, bindingsChunk, appChunk, bundlerChunk].includes(f)).map((f) => join("assets", f));

// The number must be of the production flavour: its chunk says no sentence of the development table (a build that resolved the other
// flavour, or a module that kept its prose, would measure something no production page ships).
const messages = readFileSync(join(runtime, "src", "messages.ts"), "utf8");
const sentences = [...messages.matchAll(/^ {2}\d+: "((?:[^"\\]|\\.)*)",/gm)].map((m) => JSON.parse(`"${m[1]}"`)).filter((t) => t.length >= 30 && !t.includes("{"));
const notDevelopment = (outDir, chunk) => {
  const leaked = sentences.find((t) => readFileSync(join(outDir, "assets", chunk), "utf8").includes(t));
  if (leaked !== undefined) throw new Error(`web-size-runtime: ${chunk} holds a sentence of the development flavour (${JSON.stringify(leaked)}): the package did not resolve to its production build`);
};
notDevelopment(out, runtimeChunk);

const result = { runtime: join("assets", runtimeChunk), bindings: join("assets", bindingsChunk), app: join("assets", appChunk), lazy, bundler: join("assets", bundlerChunk), modules: hello.report.modules };

if (process.env.UNDRA_SIZE_EXTRA !== "0") {
  // ----- the same page with the helper where Vite puts it ------------------------------------------------------------
  const insideDir = `${out}-with-helper`;
  const inside = await build({ label: "inside", input: helloEntry, outDir: insideDir, helper: "inside", alias: helloAlias });
  const insideChunk = inside.find("undra-runtime-");
  notDevelopment(insideDir, insideChunk);
  result.inside = fromOut(insideDir, insideChunk);

  // ----- the page that uses everything ---------------------------------------------------------------------------------
  // The playground's bindings are copied beside the entry, inside the project, so that their `@undra/runtime` resolves through the project's
  // node_modules (the linked package) as an installed app's does.
  const everything = join(project, "all-features");
  rmSync(everything, { recursive: true, force: true });
  mkdirSync(everything, { recursive: true });
  cpSync(join(REPO, "examples", "playground", "generated", "ts", "src"), join(everything, "bindings"), { recursive: true });
  copyFileSync(join(REPO, "scripts", "web-size-all-features.ts"), join(everything, "entry.ts"));
  const featuresDir = `${out}-all-features`;
  const features = await build({ label: "all-features", input: join(everything, "entry.ts"), outDir: featuresDir, helper: "beside" });
  const firstChunk = features.find("undra-runtime-");
  notDevelopment(featuresDir, firstChunk);
  const workers = new Set((features.report.workers ?? []).map((f) => f.replace(/^assets\//, "")));
  const others = [firstChunk, features.find("bindings-"), features.find("entry-"), features.find("bundler-")];
  result.allFeatures = {
    runtime: fromOut(featuresDir, firstChunk),
    lazy: features.assets.filter((f) => !others.includes(f) && !workers.has(f)).map((f) => fromOut(featuresDir, f)),
  };
}

console.log(JSON.stringify(result));
