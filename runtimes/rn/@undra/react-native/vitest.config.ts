import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

const at = (path: string): string => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig({
  resolve: {
    alias: {
      // The TypeScript runtime and the playground's bindings from their sources (as the web app
      // and the contract runner use them); React Native as a stub, since its sources do not run on
      // Node.
      "@undra/runtime": at("../../../ts/@undra/runtime/src/index.ts"),
      "@playground/core": at("../../../../examples/playground/generated/ts/src/index.ts"),
      "react-native": at("./test/support/react-native-stub.ts"),
    },
  },
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
  },
});
