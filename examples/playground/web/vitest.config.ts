import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    // As in vite.config.ts: the runtime's sources import `react` and `wa-sqlite`, and the app's copies are the ones used.
    dedupe: ["react", "react-dom", "wa-sqlite"],
    alias: {
      "@undra/runtime/react": fileURLToPath(new URL("../../../runtimes/ts/@undra/runtime/src/react.ts", import.meta.url)),
      "@undra/runtime/realtime": fileURLToPath(new URL("../../../runtimes/ts/@undra/runtime/src/realtime.ts", import.meta.url)),
      "@undra/runtime/db": fileURLToPath(new URL("../../../runtimes/ts/@undra/runtime/src/db.ts", import.meta.url)),
      "@undra/runtime": fileURLToPath(new URL("../../../runtimes/ts/@undra/runtime/src/index.ts", import.meta.url)),
      "@playground/core": fileURLToPath(new URL("../generated/ts/src/index.ts", import.meta.url)),
    },
  },
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
