import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      // The page imports the runtime's wire module from its source, the way build.sh bundles it.
      "@undra/runtime/wire": fileURLToPath(new URL("../@undra/runtime/src/wire/index.ts", import.meta.url)),
    },
  },
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
  },
});
