import { expect, test } from "vitest";
import { Probe, add, failLater } from "@playground/core";
import { boot } from "../src/harness.js";
import { counters } from "../src/stats.js";
import { sleep, step, waitFor } from "../src/wait.js";

// S06 cancellation: an AbortSignal cancels a call in the core, and the core drops the future
// (which the Probe's drop guard counts); other calls are unaffected; cancelling after completion
// is a no-op.

test("S06 cancellation", async () => {
  const { core } = await boot();
  const probe = await Probe.create(core);
  const statsBefore = await counters(core);
  const controller = new AbortController();
  const hang = probe.hang(controller.signal);
  hang.catch(() => {}); // awaited in step 2; keeps the rejection from being reported as unhandled meanwhile

  await step("1. start probe.hang() and wait until the core runs it", async () => {
    await waitFor("the hang call to start in the core", async () => (await probe.counters()).started === 1);
  });

  await step("2. cancelling ends the platform call as cancelled", async () => {
    const afterStart = await counters(core);
    expect(afterStart.activeCalls).toBeGreaterThan(statsBefore.activeCalls);
    controller.abort();
    const error = await hang.then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error, "the call rejects").toBeDefined();
    expect(error).toBe(controller.signal.reason);
    expect((error as Error).name).toBe("AbortError");
  });

  await step("3. the core dropped the future", async () => {
    const dropped = await waitFor("the core to drop the cancelled future", async () => {
      const seen = await probe.counters();
      return seen.cancelled === 1 && seen;
    });
    expect(dropped.completed).toBe(0);
    const after = await counters(core);
    expect(after.activeCalls).toBe(statsBefore.activeCalls);
    expect(after.cancelled - statsBefore.cancelled).toBe(1);
  });

  await step("4. cancelling one call leaves the other alone", async () => {
    const second = new AbortController();
    const wait = probe.wait(100);
    const other = probe.hang(second.signal);
    other.catch(() => {});
    await waitFor("both calls to start", async () => (await probe.counters()).started === 3);
    second.abort();
    expect(await wait).toBe(100);
    await other.then(
      () => {
        throw new Error("the cancelled call resolved");
      },
      () => undefined,
    );
    const seen = await waitFor("the second cancellation to be counted", async () => {
      const c = await probe.counters();
      return c.cancelled === 2 && c;
    });
    expect(seen.completed).toBe(1);
  });

  await step("5. cancelling after completion is a no-op", async () => {
    const late = new AbortController();
    expect(await probe.wait(1, late.signal)).toBe(1);
    const before = await counters(core);
    late.abort();
    // A call is handled in order: when this answers, the core has seen everything the abort could have sent.
    const seen = await probe.counters();
    expect(seen.cancelled).toBe(2);
    expect(seen.completed).toBe(2);
    const after = await counters(core);
    expect(after.cancelled - before.cancelled).toBe(0);
  });

  await step("6. a cancelled call of a method with a typed error ends as cancelled too", async () => {
    const beforeTyped = await counters(core);
    const typed = new AbortController();
    const failing = failLater(5_000, 1, core, typed.signal);
    failing.catch(() => {});
    await sleep(100);
    const abortedAt = Date.now();
    typed.abort();
    const error = await failing.then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error, "the call rejects with the signal's reason, not with a LabError").toBe(typed.signal.reason);
    expect((error as Error).name).toBe("AbortError");
    // Relative to the call's own 5 s delay, not to a second of wall clock: a cancel that waited for the core's answer would
    // end 4.9 s after the abort, so under half the delay (2.5 s) tells the two apart on a machine that stalls.
    expect(Date.now() - abortedAt, "the cancelled call ends in under half of its own 5 s delay").toBeLessThan(2_500);
    await waitFor("the cancellation of the typed call to be counted", async () => (await counters(core)).cancelled - beforeTyped.cancelled === 1);
    expect(await add(1, 1, core)).toBe(2);
  });

  probe.close();
});
