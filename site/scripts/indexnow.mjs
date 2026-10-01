#!/usr/bin/env node
// Tells IndexNow (Bing, Yandex, DuckDuckGo and others) about every URL in sitemap.xml.
// The key is the name of the site/<32 hex>.txt file, which also holds the key as its content.
//
//   node site/scripts/indexnow.mjs            submit
//   node site/scripts/indexnow.mjs --dry-run  print the JSON body and stop
import { readdirSync } from "node:fs";
import { join } from "node:path";
import { SITE, ORIGIN, read } from "./lib.mjs";

const dry = process.argv.includes("--dry-run");
const keyFile = readdirSync(SITE).find((f) => /^[0-9a-f]{32}\.txt$/.test(f));
if (!keyFile) { console.error("indexnow: no site/<32 hex>.txt key file"); process.exit(1); }
const key = keyFile.slice(0, -4);
if (read(join(SITE, keyFile)).trim() !== key) { console.error(`indexnow: ${keyFile} must contain its own name as the key`); process.exit(1); }

const urls = [...read(join(SITE, "sitemap.xml")).matchAll(/<loc>([^<]+)<\/loc>/g)].map((m) => m[1]);
const body = { host: new URL(ORIGIN).host, key, keyLocation: `${ORIGIN}${keyFile}`, urlList: urls };
if (dry) { console.log(JSON.stringify(body, null, 2)); process.exit(0); }

const res = await fetch("https://api.indexnow.org/indexnow", { method: "POST", headers: { "content-type": "application/json; charset=utf-8" }, body: JSON.stringify(body) });
console.log(`indexnow: ${urls.length} URLs submitted, HTTP ${res.status}`);
// 200 accepted, 202 accepted pending key validation; anything else is worth a look but never blocks a deploy.
process.exit(res.status === 200 || res.status === 202 ? 0 : 1);
