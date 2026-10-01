import { expect, test } from "vitest";
import { UndraCallError, UndraCoreRestarted, UndraTransportError, codecs, crashRecovery, encodeValue } from "@undra/runtime";
import { Counter, Probe, RemoteTodosQueryHandle, UndraIds, add, configureRemote, explode } from "@playground/core";
import { BASE_URL, boot } from "../src/harness.js";
import { replies } from "../src/fake-server.js";
import { Persisted } from "../src/memory-kv.js";
import { sleep, step, waitFor } from "../src/wait.js";

// S21 a trapped web core restarts from its last snapshot (ADR-049; TypeScript only): `wasm-main` with recovery on.
// A panic traps the wasm core; the runtime fails what was in flight with "restarted", instantiates the same module
// again, restores the last snapshot (stores keep their handles), observes them again and re-creates the query handles.
// The fourth trap within the window leaves the core dead.

const LIST = "s21";
const TODOS = `${BASE_URL}/lists/${LIST}/todos`;
const ITEM = { id: 1, title: "Survives", done: false };

/** Whether `error` is what a generated call fails with when the core is out of reach for `reason`. */
function unavailable(error: unknown, reason: string): boolean {
  return error instanceof UndraCallError.Unavailable && error.transport.reason === reason;
}

test("S21 a trapped web core restarts from its last snapshot", async () => {
  const restarts: UndraCoreRestarted[] = [];
  const { core, server, kv, closed, runtimeErrors } = await boot({
    load: {
      recovery: crashRecovery({ snapshotEveryMs: 50, maxRestarts: 3, perMs: 60_000 }),
      onCoreRestarted: (event) => {
        restarts.push(event);
      },
    },
  });
  await configureRemote({ baseUrl: BASE_URL }, core);
  server.on("GET", TODOS, replies.json(200, [ITEM]));
  let counter: Counter | undefined;
  let query: RemoteTodosQueryHandle | undefined;

  await step("1. a counter at 5 and the s21 query showing the server's item; a snapshot is taken", async () => {
    counter = await Counter.create(core);
    await counter.add(5);
    query = await RemoteTodosQueryHandle.create(LIST, core);
    const q = query;
    await waitFor("the query to show the item", () => q.data.peek()?.[0]?.id === 1);
    await sleep(200);
    // The query's entry is persisted (debounced 250 ms): the re-created handle reads it back after the restart.
    await waitFor("the s21 entry to be persisted", () => kv.peek(Persisted.cacheKey(UndraIds.Queries.remoteTodos, encodeValue(codecs.string, LIST))));
  });

  const probe = await Probe.create(core);
  const queryHandleBefore = (query as RemoteTodosQueryHandle).handle;

  await step("2. explode fails the call; the call in flight fails 'restarted' (Unavailable), never retried", async () => {
    const hanging = probe.hang().then(
      () => undefined,
      (e: unknown) => e,
    );
    await waitFor("the probe's call to start", async () => (await probe.counters()).started >= 1);
    const failure = await explode("kaboom", core).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(failure, "explode fails as a call").toBeInstanceOf(UndraCallError);
    const inFlight = await hanging;
    expect(unavailable(inFlight, "restarted"), `the call in flight: ${String(inFlight)}`).toBe(true);
    expect((inFlight as UndraCallError.Unavailable).transport).toBeInstanceOf(UndraTransportError);
  });

  await step("3. onCoreRestarted, once: the panic, the snapshot's age, the failed calls, the stale objects; onError the same", async () => {
    await waitFor("the restart", () => restarts.length === 1);
    const event = restarts[0] as UndraCoreRestarted;
    expect(event.report.message).toContain("kaboom");
    expect(event.restoredFromAgeMs, "restored from a snapshot").not.toBeNull();
    expect(event.restoredFromAgeMs as number, "a recent snapshot").toBeLessThan(5_000);
    expect(event.rejectedCalls).toBeGreaterThanOrEqual(1);
    expect(event.staleObjects, "the probe, which is not a store").toBeGreaterThanOrEqual(1);
    expect(runtimeErrors, "onError received the same").toEqual([event]);
    expect(runtimeErrors[0]).toBeInstanceOf(UndraCoreRestarted);
    runtimeErrors.length = 0;
    expect(closed).toEqual([]);
  });

  await step("4. the counter came back on its handle; the query handle was re-created; the probe is stale", async () => {
    const c = counter as Counter;
    expect(c.count.peek(), "restored").toBe(5);
    await c.add(1);
    await waitFor("the counter at 6", () => c.count.peek() === 6);
    const q = query as RemoteTodosQueryHandle;
    expect(q.handle, "the query handle was re-created").not.toBe(queryHandleBefore);
    expect(q.data.peek()?.map((todo) => todo.id), "the wrapper still shows the item").toEqual([1]);
    // What the core held outside its stores (the server's address) went with the instance that trapped: the app
    // configures the new one (ADR-049 3.5), then the re-created handle refetches.
    await configureRemote({ baseUrl: BASE_URL }, core);
    const gets = server.count("GET", TODOS);
    await q.invalidate();
    await waitFor("the refetch through the re-created handle", () => server.count("GET", TODOS) > gets && q.fetching.peek() === false);
    expect(q.data.peek()?.map((todo) => todo.id)).toEqual([1]);
    const stale = await probe.counters().then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(stale, "the probe refuses: a typed refusal").toBeInstanceOf(UndraCallError.Refused);
  });

  await step("5. three more traps within the minute: two more restarts, and the fourth trap leaves the core dead", async () => {
    for (let n = 2; n <= 3; n++) {
      const failure = await explode(`kaboom ${n}`, core).then(
        () => undefined,
        (e: unknown) => e,
      );
      expect(unavailable(failure, "restarted"), `trap ${n}: ${String(failure)}`).toBe(true);
      await waitFor(`restart ${n}`, () => restarts.length === n);
      expect(runtimeErrors).toEqual([restarts[n - 1]]);
      runtimeErrors.length = 0;
    }
    const last = await explode("kaboom 4", core).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(unavailable(last, "trap"), `the fourth trap is a trap: ${String(last)}`).toBe(true);
    await waitFor("the core to be dead", () => core.closed);
    expect(closed, "onClose reports the trap").toHaveLength(1);
    expect(closed[0]).toBeInstanceOf(UndraTransportError);
    expect((closed[0] as UndraTransportError).reason).toBe("trap");
    expect(restarts, "onCoreRestarted, three times in all").toHaveLength(3);
    const after = await add(1, 2, core).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(unavailable(after, "closed"), `a call on the dead core: ${String(after)}`).toBe(true);
    expect(runtimeErrors).toEqual([]);
  });
});
