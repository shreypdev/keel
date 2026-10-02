import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import { undra } from "../../../runtimes/ts/@undra/runtime/src/vite";

const here = (path: string): string => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig({
  // `undra()` builds the Rust core (`undra build --platform web`) when Vite starts, for `vite build` as for `vite dev`, and
  // under `vite dev` rebuilds it and reloads the page whenever core/src changes: there is no manual build step.
  plugins: [undra(), react()],
  resolve: {
    alias: {
      // The bindings `undra bindgen` generates, used from their TypeScript sources so there is no
      // build step between a core change and the browser.
      "@app/fieldbook-core": here("../generated/ts/src/index.ts"),
      // The Undra runtime, from the Undra checkout's sources.
      "@undra/runtime": here("../../../runtimes/ts/@undra/runtime/src/index.ts"),
    },
  },
  server: {
    // The wasm core (`undra build --platform web`) lives outside this directory.
    fs: { allow: [here(".."), here("../../../runtimes/ts/@undra/runtime")] },
  },
});
