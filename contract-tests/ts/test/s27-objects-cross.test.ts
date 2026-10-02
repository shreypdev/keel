import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { expect, test } from "vitest";
import { type AdapterOverrides, UndraCallError, type UndraCore, UndraUnhandledError, decodeSnapshot } from "@undra/runtime";
import { Shelf, Workshop, WorkshopError } from "@playground/core";
import { UndraPlaygroundA, Workshop as WorkshopA } from "@two-cores/a";
import { UndraPlaygroundB } from "@two-cores/b";
import { CapturingLog } from "../src/capturing-log.js";
import { FakeServer } from "../src/fake-server.js";
import { boot } from "../src/harness.js";
import { ManualClock } from "../src/manual-clock.js";
import { MemoryKv } from "../src/memory-kv.js";
import { counters } from "../src/stats.js";
import { step, waitFor } from "../src/wait.js";

// S27 objects cross (ADR-040): the Workshop hands out Shelf stores; one handle is one wrapper and one
// reference; children go back as parameters (borrowed); closing releases exactly one reference; a cancelled
// call owes nothing; a restore makes derived handles stale; finalizers release; an object of another core is
// refused before anything is sent.

/** What the core counts of the host's objects: its live handles and the references the host owns. */
async function refs(core: UndraCore): Promise<{ liveHandles: number; hostRefs: number }> {
  const stats = await core.stats();
  const hostRefs = stats.core?.["host_refs"];
  if (typeof hostRefs !== "number") throw new Error(`undra_stats_json has no number \`host_refs\`: ${JSON.stringify(stats.core)}`);
  expect(stats.hostRefs, "UndraStats.hostRefs is the core's host_refs").toBe(hostRefs);
  return { liveHandles: stats.liveHandles, hostRefs };
}

/** The `gc` of a Node started with `--expose-gc` (vitest.config.ts passes it), or `undefined`. */
const gc = (globalThis as { gc?: () => void }).gc;

const wasmOf = (ns: "a" | "b"): Promise<WebAssembly.Module> =>
  readFile(fileURLToPath(new URL(`../../../examples/two-cores/${ns}/build/web/playground_${ns}.wasm`, import.meta.url))).then((bytes) =>
    WebAssembly.compile(bytes),
  );

const adapters = (): AdapterOverrides => ({
  http: new FakeServer(),
  kv: new MemoryKv(),
  log: new CapturingLog(),
  clock: new ManualClock(),
  connectivity: null,
  lifecycle: null,
  secureStore: null,
  fs: null,
});

test("S27 objects cross", async () => {
  const { core, runtimeErrors } = await boot();
  /** The reports `onError` got since the last call, taken (the harness fails a scenario that leaves any). */
  const reported = (): UndraUnhandledError[] => runtimeErrors.splice(0) as UndraUnhandledError[];

  /** Every wrapper a step made stays reachable until the end: step 7 counts what one dropped wrapper gives back. */
  const kept: object[] = [];
  const start = await refs(core);
  const w = await Workshop.create(core);
  const a = await w.shelf("a");

  await step("1. a parent returns a child (a store, observed when its wrapper was made)", async () => {
    expect(a).toBeInstanceOf(Shelf);
    expect(a.core).toBe(core);
    expect(a.label.peek()).toBe("a");
    expect(a.items.peek()).toBe(0);
    const now = await refs(core);
    expect(now.liveHandles - start.liveHandles, "one for the workshop, one for the shelf").toBe(2);
    expect(now.hostRefs - start.hostRefs).toBe(2);
  });

  await step("2. one object, one handle, one wrapper; a store returned twice is mirrored once", async () => {
    const before = await refs(core);
    const again = await w.shelf("a");
    expect(again === a, "the same wrapper").toBe(true);
    expect(again.handle).toBe(a.handle);
    expect((await refs(core)).hostRefs, "the duplicate reference was given back at once").toBe(before.hostRefs);
    expect((await w.find("a")) === a).toBe(true);
    expect(await w.find("zz")).toBeNull();
    const shelves = await w.shelves();
    expect(shelves.length).toBe(1);
    expect(shelves[0] === a).toBe(true);
    const mirror = core.mirror.stats();
    await a.stock(2);
    expect(a.items.peek()).toBe(2);
    const after = core.mirror.stats();
    expect(after.changeSetsReceived - mirror.changeSetsReceived).toBe(1);
    expect(after.entriesApplied - mirror.entriesApplied, "one store, one entry applied").toBe(1);
  });

  const b = await w.shelf("b");
  await step("3. a child is passed back as a parameter (borrowed)", async () => {
    await b.stock(3);
    const before = await refs(core);
    const changeSets = core.mirror.changeSets;
    await w.merge(a, b);
    expect(a.items.peek()).toBe(0);
    expect(b.items.peek()).toBe(5);
    expect(core.mirror.changeSets - changeSets, "one change-set each, of one transaction").toBe(2);
    expect(await w.total([a, b])).toBe(5);
    expect(await w.describe(a)).toBe("a");
    expect(await w.describe(null)).toBe("none");
    expect((await refs(core)).hostRefs, "parameters are borrowed").toBe(before.hostRefs);
  });

  await step("4. closing releases exactly one reference", async () => {
    const before = await refs(core);
    const c = await w.shelf("c");
    expect((await w.shelf("c")) === c).toBe(true);
    expect((await refs(core)).hostRefs - before.hostRefs, "one wrapper, one reference").toBe(1);
    c.close();
    expect((await refs(core)).hostRefs).toBe(before.hostRefs);
    await c.stock(1); // a command: reported, not thrown
    const reports = reported();
    expect(reports.length).toBe(1);
    expect(reports[0]?.operation).toBe("Shelf.stock");
    expect(reports[0]?.error).toBeInstanceOf(UndraCallError.Refused);
    const reopened = await w.shelf("c");
    kept.push(c, reopened);
    expect(reopened === c, "a new wrapper").toBe(false);
    expect(reopened.handle).not.toBe(c.handle);
    expect(reopened.label.peek()).toBe("c");
  });

  await step("5. a call cancelled before it finishes owes nothing", async () => {
    const before = await refs(core);
    const base = (await counters(core)).activeCalls;
    const controller = new AbortController();
    const slow = w.open("slow", 60_000, controller.signal);
    slow.catch(() => {});
    await waitFor("the slow open to run in the core", async () => (await counters(core)).activeCalls > base);
    controller.abort();
    await expect(slow).rejects.toBe(controller.signal.reason);
    await waitFor("the core to drop the cancelled call", async () => (await counters(core)).activeCalls === base);
    expect(await refs(core)).toEqual(before);
    const fast = await w.open("fast", 10);
    kept.push(fast);
    expect(fast.label.peek()).toBe("fast");
    await expect(w.open("", 0)).rejects.toBeInstanceOf(WorkshopError.NoName);
    const after = await refs(core);
    expect(after.hostRefs - before.hostRefs, "only the fast shelf").toBe(1);
    expect(after.liveHandles - before.liveHandles).toBe(1);
  });

  await step("6. a restore makes derived handles stale; the method returns a fresh child", async () => {
    const snapshot = await core.snapshot();
    const handles = decodeSnapshot(snapshot).stores.map((store) => store.handle);
    expect(handles).toContain(w.handle);
    expect(handles, "a derived handle is transient").not.toContain(a.handle);
    await core.restore(snapshot);
    expect(await w.watching(), "the workshop (a store) keeps its handle and answers").toBe(0);
    await a.stock(1);
    const reports = reported();
    expect(reports.length).toBe(1);
    expect(reports[0]?.error).toBeInstanceOf(UndraCallError.Refused);
    const fresh = await w.shelf("a");
    kept.push(fresh);
    expect(fresh === a).toBe(false);
    expect(fresh.items.peek(), "the restored workshop rebuilt its shelves empty").toBe(0);
  });

  await step("7. releasing is by handle and finalizers", async () => {
    const before = await refs(core);
    // Made and dropped in a function of its own, so nothing here keeps the wrapper reachable.
    const make = async (): Promise<bigint> => (await w.shelf("dropped")).handle;
    const dropped = make();
    let closer: Shelf | undefined;
    await dropped;
    expect((await refs(core)).hostRefs - before.hostRefs).toBe(1);
    if (gc === undefined) {
      // Without `gc`, the scenario lets `close()` stand in for the finalizer (scenarios.md S27 step 7).
      closer = await w.shelf("dropped");
      closer.close();
    }
    await waitFor("the dropped wrapper's reference to come back", async () => {
      gc?.();
      await new Promise((resolve) => setTimeout(resolve, 0));
      return (await refs(core)).hostRefs === before.hostRefs;
    });
  });

  await step("8. a foreign object is refused with a typed error before anything is sent", async () => {
    const [wasmA, wasmB] = await Promise.all([wasmOf("a"), wasmOf("b")]);
    const errorsA: unknown[] = [];
    const coreA = await UndraPlaygroundA.load({ mode: "wasm-main", wasm: wasmA, shared: false, adapters: adapters(), onError: (e) => errorsA.push(e) });
    const coreB = await UndraPlaygroundB.load({ mode: "wasm-main", wasm: wasmB, shared: false, adapters: adapters() });
    try {
      // The generated classes of package A, with each core.
      const wA = await WorkshopA.create(coreA);
      const wB = await WorkshopA.create(coreB);
      const shelfOfA = await wA.shelf("x");
      const shelfOfB = await wB.shelf("x");
      expect(shelfOfB.handle, "two cores with the same history issue the same handle numbers").toBe(shelfOfA.handle);
      const [callsA, callsB] = [(await counters(coreA)).calls, (await counters(coreB)).calls];
      const [refsA, refsB] = [await refs(coreA), await refs(coreB)];
      await wA.merge(shelfOfB, shelfOfA); // a command: reported
      expect(errorsA.length).toBe(1);
      const report = errorsA[0] as UndraUnhandledError;
      expect(report.operation).toBe("Workshop.merge");
      expect(report.error).toBeInstanceOf(UndraCallError.Refused);
      expect(report.error.message).toMatch(/Shelf belongs to another core/);
      await expect(wA.total([shelfOfA, shelfOfB])).rejects.toBeInstanceOf(UndraCallError.Refused);
      await expect(wA.describe(shelfOfB)).rejects.toThrow(/Shelf belongs to another core/);
      expect((await counters(coreA)).calls, "nothing reached core A").toBe(callsA);
      expect((await counters(coreB)).calls, "nothing reached core B").toBe(callsB);
      expect(await refs(coreA)).toEqual(refsA);
      expect(await refs(coreB)).toEqual(refsB);
    } finally {
      coreA.close();
      coreB.close();
    }
  });

  await step("9. statistics: host_refs is the sum; a wrapper releases at most once", async () => {
    const before = await refs(core);
    const e = await w.shelf("e");
    expect((await refs(core)).hostRefs - before.hostRefs).toBe(1);
    e.close();
    e.close();
    const after = await refs(core);
    expect(after.hostRefs, "closed twice, released once").toBe(before.hostRefs);
    const other = await w.shelf("a");
    kept.push(other);
    expect(await w.describe(other), "the other shelves still answer").toBe("a");
  });
  expect(kept.length).toBeGreaterThan(0);
});
