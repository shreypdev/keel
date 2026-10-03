import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import { againstDist } from "../../../ts/@undra/runtime/scripts/test-dist-plugin.mjs";

const at = (path: string): string => fileURLToPath(new URL(path, import.meta.url));

// UNDRA_TS_DIST=<runtime>/dist/index.js runs the suite against the runtime's production build (ADR-057) instead of its sources.
const built = process.env.UNDRA_TS_DIST === undefined ? undefined : dirname(process.env.UNDRA_TS_DIST);
const runtime = (module: string): string => (built === undefined ? at(`../../../ts/@undra/runtime/src/${module}.ts`) : join(built, `${module}.js`));

export default defineConfig({
  plugins: built === undefined ? [] : [againstDist(built)],
  resolve: {
    alias: {
      // The TypeScript runtime and the playground's bindings from their sources (as the web app
      // and the contract runner use them); React Native as a stub, since its sources do not run on
      // Node.
      // The subpath first: an alias also matches what starts with it and a "/".
      "@undra/runtime/realtime": runtime("realtime"),
      "@undra/runtime": runtime("index"),
      "@playground/core": at("../../../../examples/playground/generated/ts/src/index.ts"),
      "react-native": at("./test/support/react-native-stub.ts"),
    },
  },
  test: {
    include: ["test/*.test.ts"],
    environment: "node",
  },
});
