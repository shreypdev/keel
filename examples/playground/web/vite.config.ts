import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const here = (path: string): string => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      // The bindings `undra bindgen` generates, used from their TypeScript sources so there is no
      // build step between a core change and the browser.
      "@playground/core": here("../generated/ts/src/index.ts"),
      // The Undra runtime, from the Undra checkout's sources: the React adapter first, because an
      // alias matches by prefix and `@undra/runtime` would swallow `@undra/runtime/react`.
      "@undra/runtime/react": here("../../../runtimes/ts/@undra/runtime/src/react.ts"),
      "@undra/runtime/realtime": here("../../../runtimes/ts/@undra/runtime/src/realtime.ts"),
      "@undra/runtime/db": here("../../../runtimes/ts/@undra/runtime/src/db.ts"),
      "@undra/runtime": here("../../../runtimes/ts/@undra/runtime/src/index.ts"),
      // The testing kit (docs/TESTING.md), for the stories.
      "@undra/testkit": here("../../../runtimes/ts/@undra/testkit/src/index.ts"),
    },
  },
  build: {
    // Two pages: the app, and the stories (`stories.html`, a plain renderer of src/stories/*.stories.tsx).
    rollupOptions: { input: { main: here("index.html"), stories: here("stories.html") } },
  },
  server: {
    // The wasm core (`undra build --platform web`) and the runtime live outside this directory.
    fs: { allow: [here("../../..")] },
  },
  // The Db port's worker (`@undra/runtime/db-worker`, wa-sqlite over OPFS) is an ES module worker, and
  // wa-sqlite finds its wasm next to its own glue, which pre-bundling would move.
  worker: { format: "es" },
  optimizeDeps: { exclude: ["wa-sqlite"] },
});
