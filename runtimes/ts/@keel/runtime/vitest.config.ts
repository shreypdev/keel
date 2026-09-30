import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
    // `global.gc` lets the tests observe the FinalizationRegistry backstop of KeelObject.
    execArgv: ["--expose-gc"],
  },
});
