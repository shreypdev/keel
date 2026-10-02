import { describe, expect, it } from "vitest";
import { benchScale, loadWebBudgets } from "../../../../../scripts/web-budgets.mjs";
import { UndraCore } from "../src/core.js";
import { WasmMainTransport } from "../src/transport/wasm-main.js";
import { CallTarget, UndraReader, UndraWriter } from "../src/wire/index.js";
import { STUB, compileStub } from "./support/stub-core.js";
import { track } from "./support/harness.js";

/*
 * The web call path's budgets are tests (constitution R9): what a call costs in JavaScript, with the core out of the
 * measurement. The core here is the stub of `support/stub-core.ts`, WebAssembly that answers a call with its own
 * arguments and nothing else, so what is timed is the runtime: the writer, the call payload, the transport's copies,
 * the reply, and the promise. The rows are `[web."node/call_async"]` and `[web."node/call_sync"]` of bench/budgets.toml;
 * the playground's device bench holds the rows of the real core to the other `[web."..."]` tables (the 5x rule of
 * that file: the budget is five times what the reference host measured, so a shared runner passes and a call path
 * that went back to allocating a promise, a DataView and a BigInt per call does not). A runner slower than the
 * reference host sets `UNDRA_BENCH_SCALE` (the workflow's 2), which multiplies these budgets as it multiplies the
 * host's (`scripts/web-budgets.mjs`); the failure message prints the scaled budget.
 */

const FREE = { target: CallTarget.FreeFunction } as const;
/** Calls per batch, batches per attempt, and attempts (the best one counts, as the host budgets' best of three). */
const BATCH = 2_000;
const BATCHES = 20;
const ATTEMPTS = 5;

async function boot(): Promise<UndraCore> {
  const transport = new WasmMainTransport({ wasm: await compileStub(), expectedSchemaHash: STUB.SCHEMA_HASH, platform: "test" });
  return track(await UndraCore.attach(transport, { expectedSchemaHash: STUB.SCHEMA_HASH, shared: false }));
}

/** The p50 over `BATCHES` batches of the per-call nanoseconds of `run`, for the best of `ATTEMPTS` attempts. */
async function bestP50(run: (calls: number) => void | Promise<void>): Promise<number> {
  let best = Number.POSITIVE_INFINITY;
  for (let attempt = 0; attempt < ATTEMPTS; attempt++) {
    await run(BATCH); // warm-up
    const samples: number[] = [];
    for (let b = 0; b < BATCHES; b++) {
      const t0 = performance.now();
      await run(BATCH);
      samples.push(((performance.now() - t0) * 1e6) / BATCH);
    }
    samples.sort((a, c) => a - c);
    best = Math.min(best, samples[samples.length >> 1] as number);
  }
  return best;
}

describe("the call path's budgets (bench/budgets.toml, [web.\"node/...\"])", () => {
  const budgets = loadWebBudgets();

  it("has a row for the asynchronous call and one for callSync", () => {
    expect(budgets["node/call_async"]?.budgetNs).toBeGreaterThan(0);
    expect(budgets["node/call_sync"]?.budgetNs).toBeGreaterThan(0);
  });

  it("an awaited call, as the generated code makes it (writer, call, reply, decode), is within its budget", async () => {
    const core = await boot();
    let sum = 0;
    const p50 = await bestP50(async (calls) => {
      for (let i = 0; i < calls; i++) {
        const w = new UndraWriter();
        w.writeU32(i & 1023);
        w.writeU32(2);
        sum += new UndraReader(await core.call(FREE, STUB.ECHO, w.finish())).readU32();
      }
    });
    expect(sum).toBeGreaterThan(0);
    const budget = budgets["node/call_async"]?.budgetNs ?? 0;
    expect(p50, `an awaited call took ${p50.toFixed(0)} ns (p50), budget ${budget.toFixed(0)} ns (scale ${benchScale()})`).toBeLessThanOrEqual(budget);
  });

  it("UndraCore.callSync is within its budget", async () => {
    const core = await boot();
    const args = new UndraWriter();
    args.writeU32(1);
    args.writeU32(2);
    const bytes = args.finish();
    let sum = 0;
    const p50 = await bestP50((calls) => {
      for (let i = 0; i < calls; i++) sum += core.callSync(FREE, STUB.ECHO, bytes).length;
    });
    expect(sum).toBeGreaterThan(0);
    const budget = budgets["node/call_sync"]?.budgetNs ?? 0;
    expect(p50, `callSync took ${p50.toFixed(0)} ns (p50), budget ${budget.toFixed(0)} ns (scale ${benchScale()})`).toBeLessThanOrEqual(budget);
  });
});
