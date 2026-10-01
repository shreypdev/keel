import { expect, test } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import type { UndraBenchApi } from "../src/bench/main";

// The device benchmark, web half: loads `bench.html` in the headless Chromium Playwright launched, runs the
// operations and the ADR-031 drain experiment on the page's wasm core, then takes N cold starts in fresh browser
// contexts, and writes what it measured to UNDRA_BENCH_RAW_OUT (the runner's half of the result file; the script
// adds the machine, the commit and the load). UNDRA_BENCH_QUICK=1 runs a fraction of a second of each part.

/** The page's API; the callbacks below run in the browser, so each reads it from `window` there. */
type Page = { undraBench: UndraBenchApi };

test("device bench, web: Chromium, wasm-main", async ({ browser, browserName }) => {
  const out = process.env["UNDRA_BENCH_RAW_OUT"] ?? "node_modules/.undra-bench-web.json";
  const quick = process.env["UNDRA_BENCH_QUICK"] === "1";
  const coldRuns = Number(process.env["UNDRA_BENCH_COLD_RUNS"] ?? (quick ? 2 : 10));

  const context = await browser.newContext();
  const page = await context.newPage();
  const problems: string[] = [];
  page.on("pageerror", (error) => problems.push(error.message));
  await page.goto("/bench.html");
  await page.waitForFunction(() => (window as unknown as { undraBench?: unknown }).undraBench !== undefined);
  await expect(page.locator("#status")).toHaveText("ready");

  const info = await page.evaluate(() => (window as unknown as Page).undraBench.info());
  const raw = await page.evaluate((q) => (window as unknown as Page).undraBench.run(q), quick);
  const reloads = await page.evaluate((n) => (window as unknown as Page).undraBench.reloads(n), quick ? 2 : 20);
  await context.close();

  // Cold starts: a fresh context each, so no cache of the module or its compiled code carries over.
  const colds: { fetch_ns: number; compile_ns: number; load_ns: number }[] = [];
  for (let i = 0; i < coldRuns; i++) {
    const fresh = await browser.newContext();
    const p = await fresh.newPage();
    await p.goto("/bench.html?cold=1");
    await p.waitForFunction(() => (window as unknown as { undraBench?: unknown }).undraBench !== undefined);
    colds.push(await p.evaluate(() => (window as unknown as Page).undraBench.cold()));
    await fresh.close();
  }

  expect(problems, "the page logged no errors").toEqual([]);
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(
    out,
    JSON.stringify(
      {
        ...raw,
        device: {
          model: `${browserName} (headless, Playwright)`,
          arch: process.arch,
          cores: info["hardware_concurrency"],
          browser: browserName,
          browser_version: browser.version(),
          headless: true,
          ...info,
        },
        cold: { launches: colds, reload_in_page: reloads },
      },
      null,
      1,
    ),
  );
  console.log(`wrote ${out}`);
});
