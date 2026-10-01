import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      // The runtime from its sources, as the web app and the contract runner use it.
      "@undra/runtime": fileURLToPath(new URL("../runtime/src/index.ts", import.meta.url)),
      // The playground's generated bindings: the recorded session of its Todos store is replayed through them.
      "@playground/core": fileURLToPath(new URL("../../../../examples/playground/generated/ts/src/index.ts", import.meta.url)),
    },
  },
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
  },
});
