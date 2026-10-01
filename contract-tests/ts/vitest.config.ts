import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import ScenarioReporter from "./src/reporter.js";

const at = (path: string): string => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig({
  resolve: {
    alias: {
      // The runtime and the generated bindings are used from their TypeScript sources, exactly as
      // the playground's web app does, so a change to either is tested without a build step.
      // The more specific alias first: the first match wins, and `@undra/runtime` would claim the subpath too.
      "@undra/runtime/worker": at("../../runtimes/ts/@undra/runtime/src/worker.ts"),
      "@undra/runtime/realtime": at("../../runtimes/ts/@undra/runtime/src/realtime.ts"),
      "@undra/runtime/db": at("../../runtimes/ts/@undra/runtime/src/db.ts"),
      "@undra/runtime": at("../../runtimes/ts/@undra/runtime/src/index.ts"),
      "@playground/core": at("../../examples/playground/generated/ts/src/index.ts"),
    },
  },
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
    testTimeout: 60_000,
    // The scenarios have real timers and quiet windows (50 ms, 200 ms, 1 s): one file at a time, so
    // a busy machine is not made busier by the scenarios themselves.
    fileParallelism: false,
    reporters: ["default", new ScenarioReporter()],
  },
});
