import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import { againstDist } from "../@undra/runtime/scripts/test-dist-plugin.mjs";

// UNDRA_TS_DIST=<runtime>/dist/index.js runs the suite against the runtime's production build (ADR-057) instead of its sources.
const wire = process.env.UNDRA_TS_DIST === undefined ? fileURLToPath(new URL("../@undra/runtime/src/wire/index.ts", import.meta.url)) : join(dirname(process.env.UNDRA_TS_DIST), "wire/index.js");

export default defineConfig({
  plugins: process.env.UNDRA_TS_DIST === undefined ? [] : [againstDist(dirname(process.env.UNDRA_TS_DIST))],
  resolve: {
    alias: {
      // The page imports the runtime's wire module from its source, the way build.sh bundles it.
      "@undra/runtime/wire": wire,
    },
  },
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
  },
});
