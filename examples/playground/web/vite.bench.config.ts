import { fileURLToPath } from "node:url";
import { defineConfig, mergeConfig } from "vite";
import base from "./vite.config";

const here = (path: string): string => fileURLToPath(new URL(path, import.meta.url));

/**
 * The device benchmark's build (`npm run bench`): `bench.html` only, into `dist-bench/`, so the benchmark page is
 * never part of the playground's build or the site. `vite preview` serves it cross-origin isolated (COOP and COEP), which
 * is what makes Chrome's `performance.now()` step 5 microseconds instead of 100.
 */
export default mergeConfig(
  base,
  defineConfig({
    build: { outDir: "dist-bench", emptyOutDir: true, rollupOptions: { input: here("bench.html") } },
    preview: { headers: { "Cross-Origin-Opener-Policy": "same-origin", "Cross-Origin-Embedder-Policy": "require-corp" } },
  }),
);
