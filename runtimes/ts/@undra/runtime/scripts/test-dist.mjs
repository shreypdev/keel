#!/usr/bin/env node
// `npm run test:dist` (ADR-057): builds the package, then runs its whole suite against the production build in `dist` (see vitest.config.ts).
// Anything after `--` goes to vitest (`npm run test:dist -- test/mirror.test.ts`).
import { spawnSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const run = (script, args, env = {}) => {
  const result = spawnSync(process.execPath, [join(ROOT, script), ...args], { cwd: ROOT, stdio: "inherit", env: { ...process.env, ...env } });
  if (result.status !== 0) process.exit(result.status ?? 1);
};

run("scripts/build.mjs", []);
run("node_modules/vitest/vitest.mjs", ["run", ...process.argv.slice(2)], { UNDRA_TEST_DIST: "1" });
