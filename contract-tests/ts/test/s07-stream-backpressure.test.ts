import { expect, test } from "vitest";
import { UndraCallError, WireError } from "@undra/runtime";
import { LabError, Probe } from "@playground/core";
import { boot } from "../src/harness.js";
import { counters } from "../src/stats.js";
import { sleep, step, waitFor } from "../src/wait.js";

// S07 stream with backpressure: the core produces items only against credit the consumer grants
// (16 at subscribe, topped up as the consumer drains), so a consumer that stops reading stops the
// producer, one that leaves early cancels it, and short streams end. A stream ends with its own
// typed error part-way, and one the core ends itself fails as cancelled by the core (ADR-036).

/** Reads `count` items from an iterator. */
async function read(iterator: AsyncIterator<number>, count: number): Promise<number[]> {
  const items: number[] = [];
  for (let i = 0; i < count; i++) {
    const next = await iterator.next();
    if (next.done === true) throw new Error(`the stream ended after ${i} of ${count} items`);
    items.push(next.value);
  }
  return items;
}

async function collect(stream: AsyncIterable<number>): Promise<number[]> {
  const items: number[] = [];
  for await (const item of stream) items.push(item);
  return items;
}

/** Reads `iterator` until it rejects, which it must do within `timeoutMs`; returns the items read on the way and the rejection. */
async function failureOf(iterator: AsyncIterator<number>, timeoutMs: number): Promise<{ items: number[]; error: unknown }> {
  const deadline = Date.now() + timeoutMs;
  const items: number[] = [];
  for (;;) {
    const left = deadline - Date.now();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<"timeout">((resolve) => {
      timer = setTimeout(() => resolve("timeout"), Math.max(left, 0));
    });
    let next: IteratorResult<number> | "timeout";
    try {
      next = await Promise.race([iterator.next(), timeout]);
    } catch (error) {
      return { items, error };
    } finally {
      clearTimeout(timer);
    }
    if (next === "timeout") throw new Error(`the stream did not fail within ${timeoutMs} ms (read ${items.length} more items)`);
    if (next.done === true) throw new Error(`the stream ended normally after ${items.length} more items instead of failing`);
    items.push(next.value);
  }
}

test("S07 stream with backpressure", async ({ task }) => {
  const { core } = await boot();
  const probe = await Probe.create(core);

  await probe.reset();
  const stream = probe.ticks(1000)[Symbol.asyncIterator]();

  await step("1. read exactly five items, then stop reading without cancelling", async () => {
    expect(await read(stream, 5)).toEqual([0, 1, 2, 3, 4]);
  });

  await step("2. for 200 ms nothing more is read, and the core did not run ahead", async () => {
    await sleep(200);
    const { produced } = await probe.counters();
    (task.meta.notes ??= []).push(`backpressure: produced ${produced} of 1000 after reading 5 and waiting 200 ms (window 5 + 64)`);
    expect(produced).toBeGreaterThanOrEqual(5);
    expect(produced).toBeLessThanOrEqual(5 + 64);
    expect(produced).toBeLessThan(1000);
  });

  await step("3. resuming reads the rest in order, the stream ends, produced is 1000", async () => {
    const rest = await read(stream, 995);
    expect(rest).toEqual(Array.from({ length: 995 }, (_, i) => i + 5));
    expect(await stream.next()).toEqual({ value: undefined, done: true });
    expect((await probe.counters()).produced).toBe(1000);
  });

  await step("4. leaving a stream early closes it and stops the producer", async () => {
    const before = await counters(core);
    const producedBefore = (await probe.counters()).produced;
    const seen: number[] = [];
    for await (const item of probe.ticks(1_000_000)) {
      seen.push(item);
      if (seen.length === 3) break;
    }
    expect(seen).toEqual([0, 1, 2]);
    await waitFor("the core to close the stream", async () => (await counters(core)).openStreams === before.openStreams, { timeoutMs: 1_000 });
    const produced = (await probe.counters()).produced - producedBefore;
    expect(produced, "items the core produced for the abandoned stream").toBeLessThan(200);
    expect((await core.stats()).openStreams, "the runtime forgot the stream too").toBe(0);
  });

  await step("5. short streams end", async () => {
    expect(await collect(probe.ticks(3))).toEqual([0, 1, 2]);
    expect(await collect(probe.ticks(0))).toEqual([]);
  });

  await step("6. a typed error part-way ends the stream with its own E, after the items before it", async () => {
    const seen: number[] = [];
    let failure: unknown;
    try {
      for await (const item of probe.ticksThenFail(5, 3, 7)) seen.push(item);
    } catch (error) {
      failure = error;
    }
    expect(seen).toEqual([0, 1, 2]);
    expect(failure, "the stream's own error, not a wire error").not.toBeInstanceOf(WireError);
    expect(failure).toBeInstanceOf(LabError);
    expect(failure).toBeInstanceOf(LabError.Rejected);
    const rejected = failure as LabError.Rejected;
    expect(rejected.kind).toBe("rejected");
    expect(rejected.code).toBe(7);
    expect(rejected.reason).toBe("stopped at 3");
    // When `fail_at` is past the end, the stream completes normally.
    expect(await collect(probe.ticksThenFail(3, 9, 7))).toEqual([0, 1, 2]);
    expect((await core.stats()).openStreams, "both streams are finished in the runtime").toBe(0);
  });

  await step("7. a stream the core cancels (a restore) fails as cancelled by the core, not as its E or a wire error", async () => {
    const bytes = await core.snapshot();
    const before = await counters(core);
    const iterator = probe.ticksThenFail(1_000_000, 999_999, 1)[Symbol.asyncIterator]();
    expect(await read(iterator, 2)).toEqual([0, 1]);
    // The probe is not a store: the restore invalidates it and ends its stream.
    await core.restore(bytes);
    const { items, error } = await failureOf(iterator, 1_000);
    // What the core had already sent against credit is still delivered first, in order.
    expect(items, "the items delivered before the failure").toEqual(Array.from({ length: items.length }, (_, i) => i + 2));
    expect(items.length).toBeLessThanOrEqual(64);
    expect(error, "not a wire decode error").not.toBeInstanceOf(WireError);
    expect(error, "not the stream's own error").not.toBeInstanceOf(LabError);
    expect(error).toBeInstanceOf(UndraCallError.CancelledByCore);
    await waitFor("the core to close the stream", async () => (await counters(core)).openStreams === before.openStreams, { timeoutMs: 1_000 });
    expect((await core.stats()).openStreams, "the runtime forgot the stream too").toBe(0);
  });

  probe.close();
});
