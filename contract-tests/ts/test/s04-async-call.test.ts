import { expect, test } from "vitest";
import { Counter, Probe, addLater } from "@playground/core";
import { boot } from "../src/harness.js";
import { step, waitFor } from "../src/wait.js";

// S04 async call: a call that waits on the core's Timer port resolves later, several at once
// resolve in the order their timers fire, and the platform may call again from inside a
// continuation or a change observer.

test("S04 async call", async () => {
  const { core } = await boot();

  await step("1. add_later(20, 22, 50) resolves to 42 after 45 ms and before 2 s", async () => {
    const started = performance.now();
    expect(await addLater(20, 22, 50, core)).toBe(42);
    const elapsedMs = performance.now() - started;
    expect(elapsedMs).toBeGreaterThanOrEqual(45);
    expect(elapsedMs).toBeLessThan(2_000);
  });

  await step("2. three concurrent calls resolve in delay order", async () => {
    const order: number[] = [];
    const values = await Promise.all(
      (
        [
          [1, 400],
          [2, 50],
          [3, 200],
        ] as const
      ).map(async ([i, delayMs]) => {
        const sum = await addLater(i, 0, delayMs, core);
        order.push(i);
        return sum;
      }),
    );
    expect(order).toEqual([2, 3, 1]);
    expect(values).toEqual([1, 2, 3]);
  });

  await step("3. Probe.wait(10) resolves to 10", async () => {
    const probe = await Probe.create(core);
    expect(await probe.wait(10)).toBe(10);
    probe.close();
  });

  await step("4a. a call made from another call's completion resolves", async () => {
    const second = await addLater(1, 2, 10, core).then((sum) => addLater(sum, 4, 10, core));
    expect(second).toBe(7);
  });

  await step("4b. a call made from inside a change observer resolves", async () => {
    const counter = await Counter.create(core);
    const followUps: Promise<number>[] = [];
    const stop = counter.count.subscribe((count) => {
      // The observer runs inside the runtime's flush; calling the core from here must not deadlock or be dropped.
      followUps.push(addLater(count, 41, 10, core));
    });
    await counter.increment();
    await waitFor("the observer's follow-up call", () => followUps.length === 1 && followUps[0]);
    expect(await followUps[0]).toBe(42);
    stop();
    counter.close();
  });
});
