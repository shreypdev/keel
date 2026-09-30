import { expect, test } from "vitest";
import { Probe } from "@playground/core";
import { boot } from "../src/harness.js";
import { counters } from "../src/stats.js";
import { sleep, step, waitFor } from "../src/wait.js";

// S07 stream with backpressure: the core produces items only against credit the consumer grants
// (16 at subscribe, topped up as the consumer drains), so a consumer that stops reading stops the
// producer, one that leaves early cancels it, and short streams end.

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

  probe.close();
});
