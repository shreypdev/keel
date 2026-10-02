#!/usr/bin/env node
// Prototype of the publish-time property mangle: every `.js` under <dist> is rewritten in place with one shared
// name cache.   node mangle-dist.mjs <runtime-dir-with-node_modules> <dist> <style: auto|under> [--extra=a,b,c] [--reserved=x,y]
import { readFileSync, writeFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
const [rt, dist, style] = process.argv.slice(2, 5).map((p, i) => (i < 2 ? resolve(p) : p));
const opt = (n, d) => { const f = process.argv.find((a) => a.startsWith(`--${n}=`)); return f ? f.slice(n.length + 3) : d; };
const { minifySync } = await import(pathToFileURL(join(rt, "node_modules/rolldown/dist/utils-index.mjs")).href);
const reserved = ["_set", "_signals", "_apply", "_observeAll", ...opt("reserved", "").split(",").filter(Boolean)];
const extra = opt("extra", "").split(",").filter(Boolean);
const files = [];
const walk = (d) => { for (const f of readdirSync(d)) { const p = join(d, f); if (statSync(p).isDirectory()) walk(p); else if (p.endsWith(".js")) files.push(p); } };
walk(dist);
// 1. inventory: every property-ish identifier that matches, with its frequency (a regex over the emitted JS is enough for a prototype).
const want = (n) => (/^_[A-Za-z]/.test(n) || extra.includes(n)) && !reserved.includes(n);
const freq = new Map();
for (const f of files) for (const m of readFileSync(f, "utf8").matchAll(/(?<![\w$])([A-Za-z_$][\w$]*)/g)) if (want(m[1])) freq.set(m[1], (freq.get(m[1]) ?? 0) + 1);
const names = [...freq.entries()].sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1)).map((e) => e[0]);
const alphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
const short = (i) => { let s = ""; do { s = alphabet[i % 52] + s; i = Math.floor(i / 52) - 1; } while (i >= 0); return s; };
let cache = {};
if (style === "under") names.forEach((n, i) => (cache[n] = `_${short(i)}`));
const singles = "ABCDEFGHIJKLMNOPQRSTUVWXYZ$";
if (style === "upper") names.forEach((n, i) => (cache[n] = i < singles.length ? singles[i] : `_${short(i - singles.length)}`));
const include = new RegExp(`^(_[A-Za-z].*${extra.length ? "|" + extra.join("|") : ""})$`);
let errors = 0;
for (const f of files) {
  const r = minifySync(f, readFileSync(f, "utf8"), { module: true, compress: false, mangle: false, mangleProps: { include, reserved, cache }, codegen: { removeWhitespace: false } });
  if (r.errors?.length) { errors++; console.error(f, r.errors[0]); continue; }
  cache = r.mangleCache ?? cache;
  writeFileSync(f, r.code);
}
writeFileSync(join(dist, "mangle-cache.json"), JSON.stringify(cache, null, 1));
console.error(`mangled ${files.length} files, ${Object.keys(cache).length} names (${errors} errors)`);
