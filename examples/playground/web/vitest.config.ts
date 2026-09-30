import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      "@keel/runtime": fileURLToPath(new URL("../../../runtimes/ts/@keel/runtime/src/index.ts", import.meta.url)),
      "@playground/core": fileURLToPath(new URL("../generated/ts/src/index.ts", import.meta.url)),
    },
  },
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
