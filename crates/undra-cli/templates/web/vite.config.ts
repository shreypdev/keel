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
      "@@TS_PACKAGE@@": here("@@GENERATED_TS_PATH@@/src/index.ts"),
@@RUNTIME_ALIAS@@    },
  },
  server: {
    // The wasm core (`undra build --platform web`) lives outside this directory.
    fs: { allow: [here("@@PROJECT_ROOT_PATH@@")@@EXTRA_FS_ALLOW@@] },
  },
});
