import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import { undra } from "@@RUNTIME_VITE_IMPORT@@";

const here = (path: string): string => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig(({ command }) => ({
  // `undra()` builds the Rust core (`undra build --platform web`) when Vite starts, for `vite build` as for `vite dev`, and
  // under `vite dev` rebuilds it whenever core/src changes and reloads the page onto it: there is no manual build step. A
  // page opened with `?undra=` runs the core `undra dev` serves instead, which keeps its state across the rebuild: not reloaded.
  plugins: [undra(), react()],
  // `vite dev` serves the debug build of the core (`@@WASM_DEBUG_PATH@@`: optimised with its DWARF line tables
  // kept), so Chrome's DevTools sets breakpoints in .rs files (it needs the C/C++ DevTools Support (DWARF) extension;
  // `undra doctor` has the link). `vite build` bundles the stripped module and never names the debug one.
  define: {
    __UNDRA_DEBUG_WASM__: JSON.stringify(command === "serve" ? "/@fs" + here("@@WASM_DEBUG_PATH@@") : ""),
  },
  resolve: {
    alias: {
      // The bindings `undra bindgen` generates, used from their TypeScript sources so there is no
      // build step between a core change and the browser.
      "@@TS_PACKAGE@@": here("@@GENERATED_TS_PATH@@/src/index.ts"),
@@RUNTIME_ALIAS@@    },
@@RUNTIME_DEDUPE@@  },
  server: {
    // The wasm core (`undra build --platform web`) lives outside this directory.
    fs: { allow: [here("@@PROJECT_ROOT_PATH@@")@@EXTRA_FS_ALLOW@@] },
  },
}));
