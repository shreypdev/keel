import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const here = (path: string): string => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      // The bindings `keel bindgen` generates, used from their TypeScript sources so there is no
      // build step between a core change and the browser.
      "@playground/core": here("../generated/ts/src/index.ts"),
      // The Keel runtime, from the Keel checkout's sources: the React adapter first, because an
      // alias matches by prefix and `@keel/runtime` would swallow `@keel/runtime/react`.
      "@keel/runtime/react": here("../../../runtimes/ts/@keel/runtime/src/react.ts"),
      "@keel/runtime": here("../../../runtimes/ts/@keel/runtime/src/index.ts"),
    },
  },
  server: {
    // The wasm core (`keel build --platform web`) and the runtime live outside this directory.
    fs: { allow: [here("../../..")] },
  },
});
