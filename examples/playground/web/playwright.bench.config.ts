import { defineConfig } from "@playwright/test";

/**
 * The device benchmark's Playwright config (`npm run bench`, `scripts/bench-device.sh --device web`): the production
 * build of `bench.html` (`vite.bench.config.ts`, cross-origin isolated so the browser's clock steps 5 microseconds, not
 * 100) in one headless Chromium, one test at a time. Set UNDRA_BROWSER_CHANNEL=chrome to use an installed Chrome.
 */
const channel = process.env["UNDRA_BROWSER_CHANNEL"];

export default defineConfig({
  testDir: "bench",
  outputDir: "node_modules/.playwright-bench-results",
  reporter: "list",
  timeout: 300_000,
  workers: 1,
  fullyParallel: false,
  use: { baseURL: "http://127.0.0.1:4174" },
  // No device descriptor: its user agent would claim to be Chrome on Windows, and the file should say what the browser is.
  projects: [{ name: "chromium", use: { browserName: "chromium", ...(channel === undefined ? {} : { channel }) } }],
  webServer: {
    command: "npm run preview:bench",
    url: "http://127.0.0.1:4174/bench.html",
    reuseExistingServer: false,
    timeout: 30_000,
  },
});
