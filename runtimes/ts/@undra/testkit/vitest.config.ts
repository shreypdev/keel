import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import { againstDist } from "../runtime/scripts/test-dist-plugin.mjs";

// UNDRA_TS_DIST=<runtime>/dist/index.js runs the suite against the runtime's production build (ADR-057) instead of its sources.
const built = process.env.UNDRA_TS_DIST;
const runtime = built ?? fileURLToPath(new URL("../runtime/src/index.ts", import.meta.url));

export default defineConfig({
  plugins: built === undefined ? [] : [againstDist(dirname(built))],
  resolve: {
    alias: {
      // The runtime from its sources, as the web app and the contract runner use it.
      "@undra/runtime": runtime,
      // The playground's generated bindings: the recorded session of its Todos store is replayed through them.
      "@playground/core": fileURLToPath(new URL("../../../../examples/playground/generated/ts/src/index.ts", import.meta.url)),
    },
  },
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
  },
});
