import { expect, test } from "vitest";
import { CallTarget, KeelReplyError, KeelWriter, ReplyStatus, codecs, decodeValue } from "@keel/runtime";
import { KeelIds } from "@playground/core";
import { boot } from "../src/harness.js";
import { counters } from "../src/stats.js";
import { step } from "../src/wait.js";

// S03 sync call: the synchronous path (`core.callSync`, the `keel_call_sync` export; `wasm-main`
// only) answers without waiting for an event loop.

const FREE = CallTarget.FreeFunction;

function addArgs(a: number, b: number): Uint8Array {
  const w = new KeelWriter();
  w.writeI32(a);
  w.writeI32(b);
  return w.finish();
}

function addLaterArgs(a: number, b: number, delayMs: number): Uint8Array {
  const w = new KeelWriter();
  w.writeI32(a);
  w.writeI32(b);
  w.writeU32(delayMs);
  return w.finish();
}

function greetArgs(name: string): Uint8Array {
  const w = new KeelWriter();
  w.writeStr(name);
  return w.finish();
}

test("S03 sync call", async ({ task }) => {
  const { core } = await boot();
  const syncAdd = (a: number, b: number): number => decodeValue(codecs.i32, core.callSync(FREE, KeelIds.Functions.add, addArgs(a, b)));

  await step("1. add and greet through the sync path", () => {
    expect(syncAdd(40, 2)).toBe(42);
    expect(syncAdd(2_147_483_647, 1)).toBe(-2_147_483_648);
    const greeting = decodeValue(codecs.string, core.callSync(FREE, KeelIds.Functions.greet, greetArgs("Ada")));
    expect(greeting).toBe("Hello, Ada, from the playground core");
  });

  await step("2. the sync path has no suspension: it returns bytes, not a promise", () => {
    const result: unknown = core.callSync(FREE, KeelIds.Functions.add, addArgs(1, 2));
    expect(result).toBeInstanceOf(Uint8Array);
    expect(typeof (result as { then?: unknown }).then).toBe("undefined");
    expect(core.mode).toBe("wasm-main");
  });

  await step("3. 10,000 sync calls: the right sums, and exactly 10,000 crossings", async () => {
    const before = await counters(core);
    const started = performance.now();
    let wrong = 0;
    for (let i = 0; i < 10_000; i++) if (syncAdd(i, 2 * i) !== 3 * i) wrong++;
    const elapsedMs = performance.now() - started;
    expect(wrong).toBe(0);
    const after = await counters(core);
    expect(after.calls - before.calls).toBe(10_000);
    const nsPerCall = Math.round((elapsedMs * 1e6) / 10_000);
    (task.meta.notes ??= []).push(`sync add: mean ${nsPerCall} ns per call over 10,000 calls (${elapsedMs.toFixed(1)} ms)`);
  });

  await step("4. an async method is refused on the sync path, and the core is unharmed", async () => {
    const before = await counters(core);
    let refusal: unknown;
    try {
      core.callSync(FREE, KeelIds.Functions.addLater, addLaterArgs(1, 1, 10));
    } catch (error) {
      refusal = error;
    }
    expect(refusal).toBeInstanceOf(KeelReplyError);
    expect((refusal as KeelReplyError).status).toBe(ReplyStatus.BadRequest);
    expect((refusal as KeelReplyError).reason).toBeTruthy();
    expect(syncAdd(1, 1)).toBe(2);
    const after = await counters(core);
    expect(after.badRequests - before.badRequests).toBe(1);
    expect(after.activeCalls).toBe(before.activeCalls);
  });
});
