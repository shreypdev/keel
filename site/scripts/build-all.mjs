#!/usr/bin/env node
// Runs every generator in order: numbers -> roadmap -> error codes -> docs navigation -> blog index (feed, sitemap) -> search index -> llms.
// CI runs this and then `git diff --exit-code site/`, so a stale committed generated file fails the build.
//
//   node site/scripts/build-all.mjs
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
for (const script of ["build-numbers", "build-roadmap", "build-errors", "sync-docs-nav", "build-blog-index", "build-search-index", "build-llms"]) {
  const r = spawnSync(process.execPath, [join(here, `${script}.mjs`)], { stdio: "inherit" });
  if (r.status !== 0) { console.error(`build-all: ${script} failed`); process.exit(r.status ?? 1); }
}
