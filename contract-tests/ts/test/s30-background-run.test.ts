import { expect, test } from "vitest";
import { UndraCallError, type UndraBackgroundReport, codecs, emitConnectivity, emitLifecycle, encodeValue } from "@undra/runtime";
import { RemoteTodosQueryHandle, UndraIds, add, configureRemote, createRemoteTodo, storageStatus } from "@playground/core";
import { BASE_URL, boot } from "../src/harness.js";
import { replies } from "../src/fake-server.js";
import { Persisted } from "../src/memory-kv.js";
import { sleep, step, waitFor } from "../src/wait.js";

// S30 background run (ADR-046). `runInBackground(deadlineMs)` over the standard function `run_background`: the core drains what
// it has queued (replay the offline queue, refetch stale queries, flush persistence) inside the window the host has, says how
// far it got, and keeps every item intact when the window closes first. In a page the runtime asks for the window itself
// (`backgroundRun`, S30 does not use it); the OS-granted windows of Swift and Kotlin call the same function.

const LIST = "s30";
const URL = `${BASE_URL}/lists/${LIST}/todos`;
const queued = { id: 9, title: "Queued", done: false };
const slow = { id: 10, title: "Slow", done: false };
const held = { id: 11, title: "Held", done: false };

/** The `Idempotency-Key`s of the POSTs whose body names `title`, oldest first. */
const keysOf = (server: { requestsTo(method: "POST", url: string): Array<{ text(): string; header(name: string): string | undefined }> }, title: string): Array<string | undefined> =>
  server
    .requestsTo("POST", URL)
    .filter((request) => request.text().includes(title))
    .map((request) => request.header("Idempotency-Key"));

test("S30 background run", async () => {
  const { core, server, kv } = await boot();
  await configureRemote({ baseUrl: BASE_URL }, core);
  server.on("GET", URL, replies.json(200, []));
  await waitFor("the offline queue to be readable", async () => (await storageStatus(core)).queueReadable);
  const handle = await RemoteTodosQueryHandle.create(LIST, core);
  await waitFor("the first list", () => handle.data.peek() !== null);
  const pendingOf = async (): Promise<number> => (await storageStatus(core)).pending;
  const outcomes = new Map<string, { readonly value: unknown } | { readonly error: unknown }>();
  /** Starts `create_remote_todo`, remembering how it ended: its rejection must never go unobserved. */
  const create = (title: string): Promise<void> =>
    createRemoteTodo(LIST, title, core).then(
      (value) => {
        outcomes.set(title, { value });
      },
      (error: unknown) => {
        outcomes.set(title, { error });
      },
    );

  let first!: Promise<void>;
  await step("1. offline work is pending: the stats say a window is worth asking for; Background writes the cache entry at once", async () => {
    emitConnectivity(core, false, "none");
    await sleep(50);
    server.on("POST", URL, replies.networkError("offline"));
    first = create("Queued");
    await waitFor("the create to be queued", async () => (await pendingOf()) === 1);
    expect(outcomes.has("Queued"), "still pending").toBe(false);
    const stats = await core.stats();
    expect(stats.background.tasks, "replay, refetch, flush").toBeGreaterThanOrEqual(3);
    expect(stats.background.pending, "a queued mutation").toBeGreaterThanOrEqual(1);
    // Going to the background writes a cache entry that waits out its 250 ms persistence debounce at once, rather than leaving it to
    // the debounce. Shown against the debounce's own clock, not a deadline of the machine's (100 ms was one, and a hosted runner
    // stalled past it): the list is fetched again with new contents, which makes its cache entry dirty and arms a debounce only
    // after `armed` was read, so a write of it seen less than 240 ms after `armed` (10 ms short, for the clocks' granularity) cannot be that debounce's. (The first fetch's own
    // debounce is waited out first, so it cannot be either.) A trial in which the machine stalled past that (the entry written
    // before Background, or seen 240 ms or more after `armed`) says nothing and is repeated with new contents, up to five times; a
    // core that leaves the entry to its debounce writes it only once the debounce could have fired, and fails all five.
    const key = Persisted.cacheKey(UndraIds.Queries.remoteTodos, encodeValue(codecs.string, LIST));
    const writes = (): number => kv.operations.filter((operation) => operation.op === "set" && operation.failure === undefined && operation.key === key).length;
    await waitFor("the first fetch's cache entry to be written by its debounce", () => writes() > 0);
    const seen: string[] = [];
    let flushed = false;
    for (let trial = 1; trial <= 5 && !flushed; trial++) {
      const title = `Server ${trial}`;
      server.on("GET", URL, replies.json(200, [{ id: 1, title, done: false }]));
      const armed = performance.now();
      const atArmed = writes();
      void handle.refetch();
      await waitFor("the refetched list", () => handle.data.peek()?.some((todo) => todo.title === title) === true);
      const before = writes();
      if (before > atArmed) {
        // Its debounce wrote it already: the machine stalled 250 ms between the refetch and here.
        seen.push(`before Background, ${(performance.now() - armed).toFixed(1)} ms`);
        continue;
      }
      emitLifecycle(core, "background");
      try {
        await waitFor("the cache entry of the list to be written after Background", () => writes() > before, { intervalMs: 1 });
      } finally {
        emitLifecycle(core, "active");
      }
      const at = performance.now() - armed;
      // 10 ms short of the debounce: the clocks the core's timer and this read can disagree by their granularity (a millisecond).
      if (at < 240) flushed = true;
      else seen.push(`${at.toFixed(1)} ms`);
    }
    server.on("GET", URL, replies.json(200, []));
    expect(flushed, `in five trials the cache entry was written only after the refetch that made it dirty (${seen.join(", ")}), when its 250 ms debounce could have fired: Background did not write it at once`).toBe(true);
  });

  await step("2. still offline, the run says so and does not wait", async () => {
    // A run that waited would return at its deadline less the half second it keeps for the host (4.5 s); under half the deadline tells
    // the two apart on a machine that stalls for a second.
    const started = performance.now();
    const report = await core.runInBackground(5_000);
    expect(performance.now() - started, "returned well before its deadline: under half of it").toBeLessThan(2_500);
    expect(report.finished).toBe(false);
    expect(report.replayed).toBe(0);
    expect(report.stillPending).toBeGreaterThanOrEqual(1);
    expect(await pendingOf(), "the item is still queued").toBe(1);
  });

  await step("3. online, the run drains the queue: replayed once, with the same Idempotency-Key", async () => {
    server.on("POST", URL, replies.json(201, queued, { delayMs: 400 }));
    emitConnectivity(core, true, "wifi");
    const report = await core.runInBackground(10_000);
    expect(report).toMatchObject({ finished: true, replayed: 1, stillPending: 0 });
    await waitFor("the pending create to resolve", () => outcomes.has("Queued"));
    await first;
    expect(outcomes.get("Queued")).toEqual({ value: queued });
    expect(await pendingOf()).toBe(0);
    const keys = keysOf(server, "Queued");
    expect(keys, "the failed attempt and the replay").toHaveLength(2);
    expect(keys[0]).toMatch(/^[0-9a-f-]{36}$/);
    expect(keys[1]).toBe(keys[0]);
  });

  let second!: Promise<void>;
  await step("4. a run cut at its deadline leaves the work intact: the replay in flight is the client's, nothing is sent twice", async () => {
    emitConnectivity(core, false, "none");
    await sleep(50);
    server.on("POST", URL, replies.networkError("offline"));
    second = create("Slow");
    await waitFor("the create to be queued", async () => (await pendingOf()) === 1);
    server.on("POST", URL, replies.json(201, slow, { delayMs: 5_000 }));
    emitConnectivity(core, true, "wifi");
    // The run is cut at its deadline less the half second it keeps for the host: measured against a timer of that length armed beside
    // it, which a slow machine fires as late as the core's, and not against 900 ms of wall clock. A run that kept no half second, or
    // waited for the POST, ends 500 ms or more after the timer.
    const started = performance.now();
    const reference = new Promise<number>((resolve) => setTimeout(() => resolve(performance.now()), 500));
    const report = await core.runInBackground(1_000);
    const ended = performance.now();
    const took = ended - started;
    expect(took, "about the deadline less the half second kept for the host").toBeGreaterThan(300);
    expect(ended - (await reference), `returned that long after a 500 ms timer armed beside it (it took ${took.toFixed(0)} ms)`).toBeLessThan(400);
    expect(report).toMatchObject({ finished: false, replayed: 0, stillPending: 1 });
    expect(await pendingOf(), "the item is still queued").toBe(1);
    expect(kv.peek(Persisted.queueKey), "the Kv queue key still holds it").toBeDefined();
    expect(Persisted.queueCount(kv.peek(Persisted.queueKey))).toBe(1);
    await waitFor("the POST to answer", () => outcomes.has("Slow"), { timeoutMs: 10_000 });
    await second;
    expect(outcomes.get("Slow")).toEqual({ value: slow });
    expect(await pendingOf()).toBe(0);
    const keys = keysOf(server, "Slow");
    expect(keys, "the failed attempt, and exactly one POST after it").toHaveLength(2);
    expect(keys[1]).toBe(keys[0]);
  });

  await step("5. a host that cancels the call: it fails with the platform's cancellation, the core works, the queue is intact", async () => {
    emitConnectivity(core, false, "none");
    await sleep(50);
    server.on("POST", URL, replies.networkError("offline"));
    const third = create("Held");
    await waitFor("the create to be queued", async () => (await pendingOf()) === 1);
    server.on("POST", URL, replies.json(201, held, { delayMs: 3_000 }));
    emitConnectivity(core, true, "wifi");
    const abort = new AbortController();
    const run = core.runInBackground(30_000, { signal: abort.signal }).then(
      (report: UndraBackgroundReport) => ({ report }),
      (error: unknown) => ({ error }),
    );
    await sleep(100);
    abort.abort();
    const outcome = await run;
    expect("error" in outcome, "the call failed").toBe(true);
    const error = (outcome as { error: unknown }).error;
    expect(error, "with cancellation, not a call failure").not.toBeInstanceOf(UndraCallError);
    expect((error as Error).name).toBe("AbortError");
    expect(await add(1, 2, core), "the core keeps working").toBe(3);
    expect(await pendingOf(), "the queue is intact while the POST is in flight").toBe(1);
    await waitFor("the POST to answer", () => outcomes.has("Held"), { timeoutMs: 10_000 });
    await third;
    expect(outcomes.get("Held")).toEqual({ value: held });
    expect(await pendingOf()).toBe(0);
  });

  await step("6. the counters: four runs, one finished, one replayed", async () => {
    const stats = await core.stats();
    expect(stats.background.runs).toBe(4);
    expect(stats.background.finished).toBe(1);
    expect(stats.background.replayed).toBe(1);
  });
});
