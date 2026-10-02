import { expect, test } from "vitest";
import { callbacks } from "@undra/runtime";
import { ReportError, type Reporter, Workshop } from "@playground/core";
import { bootWorker } from "../src/harness.js";
import { waitFor } from "../src/wait.js";

// Not a scenario: S27 and S28 run in `wasm-main`; this runs their core paths with the core in a worker (ADR-049),
// where a callback port is an asynchronous port of the main thread whose calls the worker forwards.

test("objects and host callbacks in wasm-worker mode", async () => {
  const { core } = await bootWorker();
  const w = await Workshop.create(core);

  // Objects: one wrapper per handle, a child passed back, observed through the worker.
  const a = await w.shelf("a");
  expect((await w.shelf("a")) === a).toBe(true);
  const b = await w.shelf("b");
  await a.stock(2);
  await w.merge(a, b);
  expect(b.items.peek()).toBe(2);
  expect(await w.total([a, b])).toBe(2);

  // Callbacks: ordered after the change-set before them, an async answer, a typed error, the registry let go.
  const heard: Array<{ line: string; notes: number }> = [];
  let answer: boolean | "typed" = true;
  const rep: Reporter = {
    progress: () => {},
    note: (line) => heard.push({ line, notes: w.notes.peek() }),
    confirm: () => (answer === "typed" ? Promise.reject(new ReportError.Unavailable("x")) : Promise.resolve(answer)),
  };
  const watch = await w.watch(rep);
  await w.announce("one");
  await waitFor("the note", () => heard.length === 1);
  expect(heard).toEqual([{ line: "one", notes: 1 }]);
  expect(await w.run(2, rep)).toBe(2);
  answer = "typed";
  await expect(w.run(1, rep)).rejects.toBeInstanceOf(ReportError.Unavailable);
  watch.close();
  await waitFor("the registry to let go of the reporter", () => callbacks(core).count(rep) === 0);
});
