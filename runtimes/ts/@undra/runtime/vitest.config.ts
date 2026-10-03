import { fileURLToPath } from "node:url";
import { againstDist } from "./scripts/test-dist-plugin.mjs";
import { defineConfig } from "vitest/config";

const root = fileURLToPath(new URL(".", import.meta.url));

/**
 * `UNDRA_TEST_DIST=1` (`npm run test:dist`) runs the suite against the package's production build (`dist`, ADR-057) instead of `src`:
 * the same tests, the code a production app ships: private properties renamed, modules as `tsc` wrote them. The one module that is not
 * production's is `messages.js` (scripts/test-dist-plugin.mjs says why).
 */
const dist = process.env.UNDRA_TEST_DIST !== undefined;

export default defineConfig({
  plugins: dist ? [againstDist(`${root}dist`, { sources: `${root}src` })] : [],
  resolve: {
    alias: {
      // Node would pick Solid's server build, which is not reactive; the tests of the Solid adapter need the browser one.
      "solid-js": fileURLToPath(new URL("./node_modules/solid-js/dist/solid.js", import.meta.url)),
    },
  },
  test: {
    include: ["test/**/*.test.ts"],
    // Tests of the sources themselves (the table of messages, the two flavours' modules built from them): not of a build.
    exclude: dist ? ["test/messages.test.ts", "test/flavours.test.ts"] : [],
    environment: "node",
    // `global.gc` lets the tests observe the FinalizationRegistry backstop of UndraObject.
    execArgv: ["--expose-gc"],
  },
});
