import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

const here = (path: string): string => fileURLToPath(new URL(path, import.meta.url));

// The tests use no Vite plugin: they read the wasm core from build/web (`undra build --platform web`, which
// `npm test` runs first through `globalSetup`) and import the bindings and the runtime from their sources.
export default defineConfig({
  resolve: {
    alias: {
      "@app/fieldbook-core": here("../generated/ts/src/index.ts"),
      "@undra/runtime": here("../../../runtimes/ts/@undra/runtime/src/index.ts"),
      // The testing kit (docs/TESTING.md): PreviewCore runs the real core on deterministic fakes.
      "@undra/testkit": here("../../../runtimes/ts/@undra/testkit/src/index.ts"),
    },
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
    globalSetup: ["./test-setup.ts"],
  },
});
