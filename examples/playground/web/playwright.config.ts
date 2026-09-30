import { defineConfig, devices } from "@playwright/test";

/**
 * The smoke test drives the production build (`npm run build`, served by `vite preview`) in a real
 * headless Chromium. `npx playwright install chromium` fetches the browser once; to use an installed
 * Chrome instead, set KEEL_BROWSER_CHANNEL=chrome.
 */
const channel = process.env["KEEL_BROWSER_CHANNEL"];

export default defineConfig({
  testDir: "smoke",
  // Traces and failure context go where git does not look.
  outputDir: "node_modules/.playwright-results",
  reporter: "list",
  timeout: 60_000,
  expect: { timeout: 10_000 },
  use: { baseURL: "http://127.0.0.1:4173" },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"], ...(channel === undefined ? {} : { channel }) } }],
  webServer: {
    command: "npm run preview -- --host 127.0.0.1 --port 4173 --strictPort",
    url: "http://127.0.0.1:4173",
    reuseExistingServer: false,
    timeout: 30_000,
  },
});
