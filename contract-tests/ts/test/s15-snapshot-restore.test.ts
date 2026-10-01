import { expect, test } from "vitest";
import { CallTarget, UndraCallError, UndraReplyError, UndraRestoreError, UndraUnhandledError, ReplyStatus } from "@undra/runtime";
import { BigList, Counter, Probe, UndraIds, Todos, add } from "@playground/core";
import { boot } from "../src/harness.js";
import { counters } from "../src/stats.js";
import { step, waitFor } from "../src/wait.js";

// S15 snapshot and restore: the core's state can be captured as opaque bytes and put back; the
// same handles keep working and show the captured values; identities continue; a snapshot that
// is not one is refused and changes nothing; a handle released before the snapshot stays gone.

test("S15 snapshot and restore", async () => {
  const { core, runtimeErrors } = await boot();
  const liveBefore = (await counters(core)).liveHandles;

  const todos = await Todos.create(core);
  const a = await todos.add("a");
  const b = await todos.add("b");
  await todos.toggle(b.id);
  const counter = await Counter.create(core);
  await counter.add(5);
  const list = await BigList.create(core);
  // A store that is gone before the snapshot is taken.
  const ghost = await Todos.create(core);
  await ghost.add("ghost");
  const ghostHandle = ghost.handle;
  ghost.close();

  /** What the three stores show, in a form `toEqual` can compare. */
  const view = () => ({
    todos: todos.todos.peek(),
    filter: todos.filter.peek(),
    visible: todos.visible.peek(),
    remaining: todos.remaining.peek(),
    count: counter.count.peek(),
    changes: counter.changes.peek(),
    parity: counter.parity.peek(),
    items: list.items.peek().length,
    first: list.items.peek()[0]?.id,
  });
  const atSnapshot = {
    todos: [
      { ...a, done: false },
      { ...b, done: true },
    ],
    filter: "all",
    visible: [
      { ...a, done: false },
      { ...b, done: true },
    ],
    remaining: 1,
    count: 5,
    changes: 1,
    parity: "odd",
    items: 10_000,
    first: 1,
  };
  let bytes: Uint8Array = new Uint8Array(0);

  await step("1. the stores hold what the scenario made them hold", () => {
    expect(view()).toEqual(atSnapshot);
  });

  await step("2. the snapshot is non-empty and opaque", async () => {
    bytes = await core.snapshot();
    expect(bytes.length).toBeGreaterThan(0);
  });

  await step("3. mutate every store", async () => {
    await todos.add("c");
    await todos.toggle(a.id);
    await todos.setFilter("done");
    await counter.add(10);
    await list.removeAt(0);
    expect(view()).not.toEqual(atSnapshot);
    expect(view()).toMatchObject({ filter: "done", count: 15, changes: 2, items: 9_999, first: 2 });
  });

  await step("4. restore: the same handles show the snapshot's values, delivered as change-sets", async () => {
    const changeSets = core.mirror.changeSets;
    await core.restore(bytes);
    // restore() resolves after the restored values reached the stores: no waiting.
    expect(view()).toEqual(atSnapshot);
    expect(core.mirror.changeSets - changeSets, "the restore delivered change-sets for the observed signals").toBeGreaterThanOrEqual(1);
    // And the handles are live: calls through them work.
    await counter.increment();
    expect(counter.count.peek()).toBe(6);
    await counter.add(-1);
    expect(counter.count.peek()).toBe(5);
    expect(counter.changes.peek()).toBe(3);
  });

  await step("5. identities continue above the survivors", async () => {
    const d = await todos.add("d");
    expect(d.id > b.id, `${d.id} is above ${b.id}`).toBe(true);
    expect([a.id, b.id]).not.toContain(d.id);
    expect(todos.todos.peek().map((todo) => todo.title)).toEqual(["a", "b", "d"]);
  });

  await step("6. a snapshot that is not one is refused and changes nothing", async () => {
    const junk = globalThis.crypto.getRandomValues(new Uint8Array(16));
    const shown = JSON.stringify(view());
    const liveBeforeRefusal = (await counters(core)).liveHandles;
    let refusal: unknown;
    try {
      await core.restore(junk);
    } catch (error) {
      refusal = error;
    }
    expect(refusal, `restoring ${Array.from(junk).join(",")} must fail`).toBeInstanceOf(UndraRestoreError);
    expect((refusal as UndraRestoreError).code, "the core's code for a malformed snapshot").toBe(5);
    expect(core.closed, "a refused snapshot does not take the core down").toBe(false);
    // The core still answers, and nothing was delivered for the refused bytes.
    expect(await add(1, 1, core)).toBe(2);
    core.mirror.flush();
    expect(JSON.stringify(view())).toBe(shown);
    expect((await counters(core)).liveHandles).toBe(liveBeforeRefusal);
  });

  await step("7. a handle released before the snapshot is not resurrected", async () => {
    const error = await core
      .call({ target: CallTarget.ObjectMethod, handle: ghostHandle }, UndraIds.Objects.Todos.clearDone, new Uint8Array(0))
      .then(
        () => undefined,
        (e: unknown) => e,
      );
    expect(error).toBeInstanceOf(UndraReplyError);
    expect((error as UndraReplyError).status).toBe(ReplyStatus.BadRequest);
  });

  await step("8. the restore created no handles: live_handles is the number of surviving stores", async () => {
    expect((await counters(core)).liveHandles - liveBefore).toBe(3);
  });

  await step("9. a call in flight across a restore ends as cancelled by the core, and the invalidated object refuses", async () => {
    const probe = await Probe.create(core);
    const hanging = probe.hang();
    hanging.catch(() => {}); // awaited below
    await waitFor("the hang call to start in the core", async () => (await probe.counters()).started === 1);
    await core.restore(await core.snapshot());
    const error = await hanging.then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error, "the call fails as cancelled by the core, not as an abort").toBeInstanceOf(UndraCallError.CancelledByCore);
    expect((error as UndraCallError.CancelledByCore).kind).toBe("cancelledByCore");
    // The probe is not a store, so the restore invalidated its handle: a call on it is refused (a bad request), and the
    // command `reset()` resolves and reports to `onError`.
    const refused = await probe.counters().then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(refused).toBeInstanceOf(UndraCallError.Refused);
    expect((refused as UndraCallError.Refused).reason).toBeTruthy();
    runtimeErrors.length = 0;
    await probe.reset();
    const reports = runtimeErrors.splice(0) as UndraUnhandledError[];
    expect(reports.map((r) => r.operation)).toEqual(["Probe.reset"]);
    expect(reports[0]?.error).toBeInstanceOf(UndraCallError.Refused);
    probe.close();
  });

  await step("10. a stream in flight across a restore ends as cancelled by the core", async () => {
    const streamed = await Probe.create(core);
    // Read one item, then stop reading without cancelling (as S07 does): the credit window keeps the core from running ahead,
    // and the stream is still open when the restore arrives.
    const iterator = streamed.ticks(1_000_000)[Symbol.asyncIterator]();
    expect((await iterator.next()).done).toBe(false);
    await core.restore(await core.snapshot());
    // What the core sent before the restore is still delivered; then the stream ends with the core's own String.
    let outcome: unknown;
    try {
      for (let next = await iterator.next(); !next.done; next = await iterator.next()) continue;
    } catch (error) {
      outcome = error;
    }
    expect(outcome, "the core's own String is not read as a typed error").toBeInstanceOf(UndraCallError.CancelledByCore);
    streamed.close();
  });

  todos.close();
  counter.close();
  list.close();
});
