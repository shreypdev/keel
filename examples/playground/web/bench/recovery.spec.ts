import { expect, test } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import type { UndraBenchApi } from "../src/bench/main";

// The web recovery rows of ADR-049 (`npm run bench:recovery`): loads `bench.html` in the headless Chromium Playwright
// launched (cross-origin isolated, so the clock steps 5 microseconds) and runs `ts/snapshot_take_100kb` and
// `ts/recovery_restart_100kb` on a core of their own, `UNDRA_BENCH_RECOVERY_RUNS` times (default 3) in fresh pages.
// Writes every run to UNDRA_BENCH_RECOVERY_OUT and prints the p50 and p99 of each. UNDRA_BENCH_QUICK=1 for a smoke run.

type Page = { undraBench: UndraBenchApi };

test("recovery bench, web: Chromium, wasm-main", async ({ browser, browserName }) => {
  const out = process.env["UNDRA_BENCH_RECOVERY_OUT"] ?? "node_modules/.undra-bench-recovery.json";
  const quick = process.env["UNDRA_BENCH_QUICK"] === "1";
  const runs = Number(process.env["UNDRA_BENCH_RECOVERY_RUNS"] ?? (quick ? 1 : 3));
  const results = [];
  let info: Record<string, unknown> = {};
  for (let run = 0; run < runs; run++) {
    const context = await browser.newContext();
    const page = await context.newPage();
    const problems: string[] = [];
    page.on("pageerror", (error) => problems.push(error.message));
    await page.goto("/bench.html");
    await page.waitForFunction(() => (window as unknown as { undraBench?: unknown }).undraBench !== undefined);
    info = await page.evaluate(() => (window as unknown as Page).undraBench.info());
    results.push(await page.evaluate((q) => (window as unknown as Page).undraBench.recovery(q), quick));
    expect(problems, "the page logged no errors").toEqual([]);
    await context.close();
  }
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(
    out,
    JSON.stringify({ device: { model: `${browserName} (headless, Playwright)`, browser_version: browser.version(), arch: process.arch, ...info }, runs: results }, null, 1),
  );
  for (const [i, result] of results.entries()) {
    for (const op of result.ops) {
      const ms = (ns: number): string => (ns / 1e6).toFixed(3);
      console.log(`run ${i + 1}: ${op.id}: p50 ${ms(op.p50)} ms, p99 ${ms(op.p99)} ms, mean ${ms(op.mean)} ms (${result.snapshot_bytes} bytes, ${result.signals} signals)`);
    }
  }
  console.log(`wrote ${out}`);
  // ADR-049's budgets (desktop Chromium) are tests (R9): every run's p50 is within its row's budget. A quick
  // smoke run is too short to judge.
  if (!quick) {
    for (const result of results) {
      for (const op of result.ops) {
        const budgetMs = BUDGET_MS[op.id];
        if (budgetMs !== undefined) expect(op.p50 / 1e6, `${op.id} p50 (budget ${budgetMs} ms)`).toBeLessThan(budgetMs);
      }
    }
  }
});

/** ADR-049's budget per row, in milliseconds (`bench/RESULTS.md`, "Web recovery"). */
const BUDGET_MS: Record<string, number> = {
  "ts/snapshot_take_100kb": 2,
  "ts/recovery_restart_100kb": 50,
};
