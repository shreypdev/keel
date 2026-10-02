import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import ScenarioReporter from "../../../../contract-tests/ts/src/reporter.js";

const at = (path: string): string => fileURLToPath(new URL(path, import.meta.url));

/*
 * The contract scenarios (contract-tests/scenarios.md) through @undra/react-native's JavaScript:
 * contract-tests/ts's scenario files, unchanged, with their harness swapped for one that attaches
 * every core through `NativeTransport` over a stand-in of the native module (test/contract/). What
 * this column proves and what only the app can (JSI, Hermes, invokeAsync, the C++ host, the native
 * frame source, native panic containment) is in docs/REACT_NATIVE.md, "What is tested where".
 *
 *   npm run test:contract        # prints SCENARIO Sxx lines; contract-tests/check.sh rn grades them
 */
export default defineConfig({
  resolve: {
    alias: [
      { find: /^\.\.\/src\/harness\.js$/, replacement: at("./test/contract/harness.ts") },
      { find: /^\.\.\/src\/wasm-exports\.js$/, replacement: at("./test/contract/wasm-exports.ts") },
      { find: "@undra/runtime/realtime", replacement: at("../../../ts/@undra/runtime/src/realtime.ts") },
      { find: "@undra/runtime", replacement: at("../../../ts/@undra/runtime/src/index.ts") },
      { find: "@playground/core", replacement: at("../../../../examples/playground/generated/ts/src/index.ts") },
      // S27 step 8: the playground core under the two namespaces of S26 (ADR-044), loaded in wasm-main beside the harness's.
      { find: "@two-cores/a", replacement: at("../../../../examples/two-cores/a/generated/ts/src/index.ts") },
      { find: "@two-cores/b", replacement: at("../../../../examples/two-cores/b/generated/ts/src/index.ts") },
      { find: /^vitest$/, replacement: at("./node_modules/vitest/dist/index.js") },
    ],
  },
  test: {
    root: at("../../../.."),
    include: [
      "contract-tests/ts/test/s{01,02,03,04,05,06,07,08,09,10,11,12,13,14,15,16,18,19,27,28,31}-*.test.ts",
      "runtimes/rn/@undra/react-native/test/contract/s17-native.test.ts",
    ],
    environment: "node",
    testTimeout: 60_000,
    fileParallelism: false,
    // S27 and S28 collect garbage to watch finalizers and the callback registry let go.
    execArgv: ["--expose-gc"],
    // The reporter is typed against contract-tests/ts's own vitest install; the shape is the same.
    reporters: ["default", new ScenarioReporter() as never],
  },
});
