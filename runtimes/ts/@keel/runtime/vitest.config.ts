import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      // Node would pick Solid's server build, which is not reactive; the tests of the Solid adapter need the browser one.
      "solid-js": fileURLToPath(new URL("./node_modules/solid-js/dist/solid.js", import.meta.url)),
    },
  },
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
    // `global.gc` lets the tests observe the FinalizationRegistry backstop of KeelObject.
    execArgv: ["--expose-gc"],
  },
});
