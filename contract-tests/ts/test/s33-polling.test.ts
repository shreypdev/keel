import { expect, test } from "vitest";
import { emitConnectivity, emitLifecycle } from "@undra/runtime";
import { TickError, TickerQueryHandle, setTickerFailing, tickerFetches } from "@playground/core";
import { boot } from "../src/harness.js";
import { sleep, step, waitFor } from "../src/wait.js";

// S33 polling (ADR-043): the `ticker` query has `interval = "1s"` and returns a counter that every fetch bumps. The core's Timer is the
// runtime's real one (the harness's Clock is manual, but `ctx.sleep` is not), so this scenario takes real seconds: the waits are the
// scenario's (2.5 s for "advances", 1.5 s for "does not move"). Lifecycle and Connectivity are sent as events, as S14 sends Connectivity.

test("S33 polling", { timeout: 90_000 }, async () => {
  const { core } = await boot();
  const ticker = await TickerQueryHandle.create(core);
  /** When each value of `data` arrived on this thread (a monotonic clock), oldest first. */
  const changes: Array<{ readonly at: number; readonly value: number | null }> = [];
  ticker.data.subscribe((value) => changes.push({ at: performance.now(), value }));

  /** The time of the next change of `data` after the ones seen so far, waiting at most `ms`. */
  const nextChange = async (what: string, ms: number): Promise<number> => {
    const seen = changes.length;
    return (await waitFor(what, () => changes[seen]?.at, { timeoutMs: ms })) as number;
  };

  await step("1. a poll after each fetch, an interval after the end of the last", async () => {
    await waitFor("the first fetch", () => ticker.data.peek() === 1, { timeoutMs: 1_000 });
    await waitFor("the second fetch within 2.5 s", () => ticker.data.peek() === 2, { timeoutMs: 2_500 });
    expect(await tickerFetches(core)).toBeGreaterThanOrEqual(2);
    // Two consecutive fetches are at least 0.9 s apart: the interval runs from the end of a fetch.
    const second = changes.find((change) => change.value === 2) as { at: number };
    const third = await nextChange("the third fetch", 2_500);
    expect(third - second.at, "ms between two fetches").toBeGreaterThanOrEqual(900);
  });

  await step("2. Background pauses polling, Active resumes it", async () => {
    emitLifecycle(core, "background");
    await sleep(100);
    const before = await tickerFetches(core);
    await sleep(1_500);
    expect(await tickerFetches(core), "nothing is fetched in the background").toBe(before);
    const shown = ticker.data.peek() as number;
    emitLifecycle(core, "active");
    await waitFor("data to advance after Active", () => (ticker.data.peek() ?? 0) > shown, { timeoutMs: 2_500 });
  });

  await step("3. offline pauses polling, online resumes it", async () => {
    emitConnectivity(core, false, "none");
    await sleep(100);
    const before = await tickerFetches(core);
    await sleep(1_500);
    expect(await tickerFetches(core), "nothing is fetched while offline").toBe(before);
    const shown = ticker.data.peek() as number;
    emitConnectivity(core, true, "wifi");
    await waitFor("data to advance when online", () => (ticker.data.peek() ?? 0) > shown, { timeoutMs: 2_500 });
  });

  await step("4. a failure keeps polling and clears on success", async () => {
    await setTickerFailing(true, core);
    await waitFor("the failure", () => ticker.error.peek() instanceof TickError.Failing, { timeoutMs: 2_500 });
    const failing = await tickerFetches(core);
    await sleep(1_500);
    expect(await tickerFetches(core), "the failing query keeps being fetched").toBeGreaterThan(failing);
    const shown = ticker.data.peek() as number;
    await setTickerFailing(false, core);
    await waitFor("the error to clear", () => ticker.error.peek() === null, { timeoutMs: 2_500 });
    await waitFor("data to advance", () => (ticker.data.peek() ?? 0) > shown, { timeoutMs: 2_500 });
  });

  await step("5. an observer's override: 3 s, then back to the query's own second", async () => {
    await ticker.setPollInterval(3_000);
    // The fetch that was already scheduled ends; the gap after it is the override's.
    const ended = await nextChange("the fetch that was scheduled before the override", 4_000);
    const next = await nextChange("the fetch after it", 4_000);
    expect(next - ended, "ms between the two fetches with a 3 s interval").toBeGreaterThanOrEqual(2_900);

    await ticker.setPollInterval(null);
    const cleared = await nextChange("the fetch the 3 s interval had scheduled", 4_000);
    const following = await nextChange("the fetch after it", 2_500);
    expect(following - cleared, "ms between two fetches with the query's own interval").toBeLessThan(2_000);
    expect(following - cleared).toBeGreaterThanOrEqual(900);
  });

  await step("6. the last observer stops it", async () => {
    ticker.close();
    await sleep(200);
    const stopped = await tickerFetches(core);
    await sleep(2_500);
    expect(await tickerFetches(core), "nobody is watching, nobody polls").toBe(stopped);
  });
});
