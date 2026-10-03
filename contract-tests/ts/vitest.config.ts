import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import { againstDist } from "../../runtimes/ts/@undra/runtime/scripts/test-dist-plugin.mjs";
import ScenarioReporter from "./src/reporter.js";

const at = (path: string): string => fileURLToPath(new URL(path, import.meta.url));

// UNDRA_TS_DIST=<runtime>/dist/index.js runs the scenarios against the runtime's production build (ADR-057: renamed private properties)
// instead of its sources: what an app ships. Run `npm run build` in runtimes/ts/@undra/runtime first.
const built = process.env.UNDRA_TS_DIST === undefined ? undefined : dirname(process.env.UNDRA_TS_DIST);
const runtime = (module: string): string => (built === undefined ? at(`../../runtimes/ts/@undra/runtime/src/${module}.ts`) : join(built, `${module}.js`));

export default defineConfig({
  plugins: built === undefined ? [] : [againstDist(built)],
  resolve: {
    alias: {
      // The runtime and the generated bindings are used from their TypeScript sources, exactly as
      // the playground's web app does, so a change to either is tested without a build step.
      // The more specific alias first: the first match wins, and `@undra/runtime` would claim the subpath too.
      "@undra/runtime/worker": runtime("worker"),
      "@undra/runtime/realtime": runtime("realtime"),
      "@undra/runtime/db-worker": runtime("db-worker"),
      "@undra/runtime/db": runtime("db"),
      "@undra/runtime": runtime("index"),
      "@undra/testkit": at("../../runtimes/ts/@undra/testkit/src/index.ts"),
      "@playground/core": at("../../examples/playground/generated/ts/src/index.ts"),
      // S26: the playground core under two more namespaces (ADR-044), each with its own bindings.
      "@two-cores/a": at("../../examples/two-cores/a/generated/ts/src/index.ts"),
      "@two-cores/b": at("../../examples/two-cores/b/generated/ts/src/index.ts"),
    },
  },
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
    testTimeout: 60_000,
    // The scenarios have real timers and quiet windows (50 ms, 200 ms, 1 s): one file at a time, so
    // a busy machine is not made busier by the scenarios themselves.
    fileParallelism: false,
    // S27 and S28 collect garbage to watch finalizers release references and the registry let go of a reporter.
    execArgv: ["--expose-gc"],
    reporters: ["default", new ScenarioReporter()],
  },
});
