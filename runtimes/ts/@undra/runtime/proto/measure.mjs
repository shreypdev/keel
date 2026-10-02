#!/usr/bin/env node
// The gate's build (scripts/web-size-runtime.mjs), parameterised for the 16 KB design work.
//   node measure.mjs <project> <runtime-dir> <out> [--entry=src/index.ts|dist/index.js] [--map] [--no-minify]
//                    [--mangle-props=<regex>] [--reserved=a,b] [--alias name=path ...] [--define k=v] [--conditions=a,b]
// Prints JSON { runtime: {file, bytes, gz}, bindings, app, lazy: [...] } with zlib level 9 sizes (python-compatible: node's
// zlib at level 9 / memLevel 8 gives the same deflate stream as Python's gzip.compress(data, 9); the gzip header is 10
// bytes + 8 trailer in both).
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { execFileSync } from "node:child_process";

const args = process.argv.slice(2);
const pos = args.filter((a) => !a.startsWith("--"));
const flag = (name) => args.find((a) => a === `--${name}` || a.startsWith(`--${name}=`));
const val = (name, d) => {
  const f = flag(name);
  return f === undefined ? d : f.includes("=") ? f.slice(f.indexOf("=") + 1) : true;
};
const [project, runtime, out] = pos.map((p) => resolve(p));
const viteHome = val("vite-from", runtime);
const vite = await import(pathToFileURL(join(resolve(viteHome), "node_modules", "vite", "dist", "node", "index.js")).href);
const bindings = JSON.parse(readFileSync(join(project, "generated", "ts", "package.json"), "utf8")).name;
const entry = val("entry", "src/index.ts");
const aliases = { [bindings]: join(project, "generated", "ts", "src", "index.ts"), "@undra/runtime": join(runtime, entry) };
for (const a of args.filter((x) => x.startsWith("--alias="))) {
  const [k, v] = a.slice(8).split("=");
  aliases[k] = resolve(v);
}
const define = {};
for (const a of args.filter((x) => x.startsWith("--define="))) {
  const [k, v] = a.slice(9).split("=");
  define[k] = v;
}
const mangle = val("mangle-props");
const reserved = (val("reserved", "") || "").split(",").filter(Boolean);
const minify =
  flag("no-minify") !== undefined
    ? false
    : mangle
      ? { mangleProps: { include: new RegExp(mangle), reserved } }
      : true;
const inputFile = val("input", join(project, "web", "src", "undra.ts"));

const moduleReport = {
  name: "undra-size-modules",
  generateBundle(_o, bundle) {
    if (!process.env.UNDRA_SIZE_MODULES) return;
    for (const chunk of Object.values(bundle)) {
      if (chunk.type !== "chunk") continue;
      console.error(`chunk ${chunk.fileName} (${chunk.code.length} bytes)`);
      const rows = Object.entries(chunk.modules)
        .map(([id, m]) => [m.renderedLength, id.replace(/^.*[\\/](?=runtimes[\\/]|generated[\\/]|web[\\/])/, ""), m.renderedExports ?? []])
        .sort((a, b) => b[0] - a[0]);
      for (const [length, id, exports] of rows) if (length > 0) console.error(`  ${String(length).padStart(7)}  ${id}${process.env.UNDRA_SIZE_MODULES === "exports" ? `  [${exports.join(" ")}]` : ""}`);
    }
  },
};

const extraPlugins = [];
if (flag("mangle") !== undefined || flag("messages") !== undefined) {
  const proto = await import(pathToFileURL(join(import.meta.dirname, "proto-plugins.mjs")).href);
  if (flag("mangle") !== undefined) extraPlugins.push(await proto.mangle(runtime, { style: val("mangle") === true ? "under" : val("mangle"), extra: (val("mangle-extra", "") || "").split(",").filter(Boolean), dump: join(out, "..", "mangle-cache.json") }));
  if (flag("messages") !== undefined) extraPlugins.push(proto.messages(runtime, { helper: entry.startsWith("dist") ? join(runtime, "dist", "msg.js") : join(runtime, "src", "msg.ts"), dump: join(out, "..", "messages.json"), keep: ["The operation was aborted"] }));
}
const single = flag("single") !== undefined; // one JS chunk (mangleProps in Rolldown needs it): everything inlined
await vite.build({
  configFile: false,
  plugins: [moduleReport, ...extraPlugins],
  root: join(project, "web"),
  logLevel: "error",
  mode: "production",
  define,
  resolve: { alias: aliases, ...(val("conditions") ? { conditions: String(val("conditions")).split(",") } : {}) },
  build: {
    outDir: out,
    emptyOutDir: true,
    assetsInlineLimit: 0,
    ...(flag("no-preload") !== undefined ? { modulePreload: false } : {}),
    minify: minify === true ? true : minify === false ? false : "oxc",
    sourcemap: flag("map") !== undefined ? "hidden" : false,
    ...(val("target") ? { target: val("target") } : {}),
    rollupOptions: {
      input: resolve(String(inputFile)),
      preserveEntrySignatures: "exports-only",
      output: {
        ...(typeof minify === "object" ? { minify } : {}),
        ...(single
          ? { inlineDynamicImports: true }
          : {
              codeSplitting: {
                groups: [
                  { name: "undra-runtime", test: (id) => id.startsWith(runtime + "/") && !id.includes("/node_modules/"), tags: ["$initial"] },
                  { name: "bindings", test: /generated[\\/]ts[\\/]/ },
                  ...(flag("merge-lazy") !== undefined
                    ? [
                        { name: "framed", test: /(wire[\\/](kind|session|envelope|framed-payloads)|transport[\\/]framed|mirror-waiters)\.(ts|js)$/, priority: 5 },
                        { name: "extras", test: /[\\/](extras|background|wasm-snapshot)\.(ts|js)$/, priority: 5 },
                        { name: "standard", test: /adapters[\\/](standard|ports|codecs|types|secure|fs|kv|http|idb|ids)\.(ts|js)$|[\\/]fnv\.(ts|js)$/, priority: 5 },
                      ]
                    : []),
                  ...(flag("split-helper") !== undefined ? [{ name: "vite-preload", test: /vite[\\/]preload-helper/, priority: 10 }] : []),
                ],
              },
            }),
      },
    },
  },
});

const gz = (file) => {
  const data = readFileSync(join(out, file));
  // The gate's compressor: Python's zlib at level 9 (scripts/wasm-size.sh).
  const n = Number(execFileSync("python3", ["-c", "import gzip,sys;print(len(gzip.compress(open(sys.argv[1],'rb').read(),9,mtime=0)))", join(out, file)]).toString());
  return { file, bytes: data.length, gz: n };
};
const assets = readdirSync(join(out, "assets")).filter((f) => f.endsWith(".js"));
if (single) {
  console.log(JSON.stringify({ all: assets.map((f) => gz(join("assets", f))) }));
} else {
  const find = (prefix, not) => {
    const hits = assets.filter((f) => f.startsWith(prefix) && !(not && f.startsWith(not)));
    if (hits.length !== 1) throw new Error(`expected one ${prefix}*.js chunk, found ${JSON.stringify(hits)}`);
    return join("assets", hits[0]);
  };
  const r = find("undra-runtime-");
  const b = find("bindings-");
  const a = find("undra-", "undra-runtime-");
  const lazy = assets.map((f) => join("assets", f)).filter((f) => ![r, b, a].includes(f));
  console.log(JSON.stringify({ runtime: gz(r), bindings: gz(b), app: gz(a), lazy: lazy.map(gz), lazyGz: lazy.map(gz).reduce((s, x) => s + x.gz, 0) }));
}
