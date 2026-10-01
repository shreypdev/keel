import { expect, test } from "vitest";
import { StorageError, codecs, emitConnectivity, emitLifecycle, encodeValue } from "@undra/runtime";
import { type RemoteTodo, RemoteTodosQueryHandle, UndraIds, add, configureRemote, saveNote, storageStatus } from "@playground/core";
import { BASE_URL, boot } from "../src/harness.js";
import { FakeServer, replies } from "../src/fake-server.js";
import { MemoryKv, Persisted } from "../src/memory-kv.js";
import { counters } from "../src/stats.js";
import { sleep, step, waitFor } from "../src/wait.js";

// S19 storage failures are typed (ADR-049): the storage ports have an error channel, so an adapter that cannot
// store answers a `StorageError` and the core neither panics nor traps (a wasm core would have trapped before).
// The TypeScript variant runs every step, steps 3 and 4 on fresh cores.

const LIST = "s19";
const URL = `${BASE_URL}/lists/${LIST}/todos`;
const NOTES = `${BASE_URL}/lists/${LIST}/notes`;
const item: RemoteTodo = { id: 1, title: "Stored", done: false };
/** The key of the `s19` entry of `remote_todos`: the query id and the fnv1a64 of its encoded argument. */
const ENTRY_KEY = Persisted.cacheKey(UndraIds.Queries.remoteTodos, encodeValue(codecs.string, LIST));

test("S19 storage failures are typed", async () => {
  const { core, server, kv, log } = await boot();
  await configureRemote({ baseUrl: BASE_URL }, core);
  server.on("GET", URL, replies.json(200, [item]));
  const panicsBefore = (await counters(core)).panics;
  const statusBefore = await storageStatus(core);
  const logged = log.records.length;
  let handle: RemoteTodosQueryHandle | undefined;

  await step("1. every write fails Full: the item shows, nothing of it is stored, one WARN says so, nothing panicked", async () => {
    kv.fail("set", new StorageError.Full());
    handle = await RemoteTodosQueryHandle.create(LIST, core);
    const h = handle;
    await waitFor("the s19 item to show", () => h.data.peek()?.[0]?.id === 1 && !h.fetching.peek());
    await sleep(300); // the write is debounced 250 ms
    await waitFor("the failed write to be counted", async () => (await storageStatus(core)).writeFailed > statusBefore.writeFailed);
    expect(kv.peek(ENTRY_KEY), `${ENTRY_KEY} is not in the Kv`).toBeUndefined();
    // In a fresh core the description of the entry's type is written first, and that write fails: the entry's own write
    // never starts (on Swift and Kotlin the description was stored by an earlier scenario, so the entry's write fails).
    expect(
      kv.operations.some((o) => o.op === "set" && o.failure instanceof StorageError.Full),
      "a write was attempted and failed Full",
    ).toBe(true);
    const warnings = log.records.slice(logged).filter((r) => r.level === 3 && r.target === "undra::query" && r.message.includes("storage is full"));
    expect(warnings, "WARN records of undra::query saying the storage is full").toHaveLength(1);
    expect((await counters(core)).panics, "no panic").toBe(panicsBefore);
    expect(core.closed).toBe(false);
    expect(await add(1, 2, core), "the core answers the next call").toBe(3);
  });

  await step("2. the Kv heals: a refetch stores the entry, format 2, next to the description of its type", async () => {
    kv.heal();
    const h = handle as RemoteTodosQueryHandle;
    await h.invalidate();
    const stored = await waitFor("the s19 entry to be stored", () => kv.peek(ENTRY_KEY));
    expect([...stored.subarray(0, 2)], "the format of the stored entry").toEqual([2, 0]);
    const fingerprint = Persisted.fingerprint(stored, 10) as bigint;
    expect(kv.peek(Persisted.typesKey(fingerprint)), `the Kv holds ${Persisted.typesKey(fingerprint)}`).toBeDefined();
    await waitFor("the data to still show", () => h.data.peek()?.[0]?.id === 1 && !h.fetching.peek());
  });

  await step("3. a failed read of a cache entry starts it empty: no data until it fetched, and the key stays", async () => {
    const kv3 = new MemoryKv(kv.entries());
    kv3.fail("get", new StorageError.Io("busy"), { key: ENTRY_KEY, times: 1 });
    const server3 = new FakeServer();
    let answeredAt = Number.POSITIVE_INFINITY;
    server3.on("GET", URL, () => {
      answeredAt = Date.now() + 300;
      return replies.json(200, [item], { delayMs: 300 });
    });
    const fresh = await boot({ kv: kv3, server: server3 });
    await configureRemote({ baseUrl: BASE_URL }, fresh.core);
    const h = await RemoteTodosQueryHandle.create(LIST, fresh.core);
    let firstShownAt: number | undefined;
    const stop = h.data.subscribe((data) => {
      if (data !== null && data.length > 0 && firstShownAt === undefined) firstShownAt = Date.now();
    });
    if ((h.data.peek()?.length ?? 0) > 0) firstShownAt ??= Date.now();
    await waitFor("the fetched item to show", () => h.data.peek()?.[0]?.id === 1);
    stop();
    expect(
      kv3.operations.some((o) => o.op === "get" && o.key === ENTRY_KEY && o.failure instanceof StorageError.Io),
      "the read of the entry failed Io",
    ).toBe(true);
    expect(server3.count("GET", URL), "the entry was fetched").toBeGreaterThanOrEqual(1);
    expect(firstShownAt ?? 0, "no data showed before the fetch answered").toBeGreaterThanOrEqual(answeredAt - 5);
    expect(kv3.peek(ENTRY_KEY), "the entry's key is still in the Kv").toBeDefined();
    h.close();
    fresh.core.close();
    expect(fresh.core.closed).toBe(true);
  });

  await step("4. an unreadable queue is never overwritten, and replays once readable", async () => {
    const kv4 = new MemoryKv();
    kv4.fail("get", new StorageError.Locked(), { key: Persisted.queueKey });
    const fresh = await boot({ kv: kv4 });
    emitConnectivity(fresh.core, false, "none");
    await configureRemote({ baseUrl: BASE_URL }, fresh.core);
    await waitFor("the failed read of the queue", () => kv4.operations.some((o) => o.op === "get" && o.key === Persisted.queueKey && o.failure !== undefined));
    expect((await storageStatus(fresh.core)).queueReadable).toBe(false);
    fresh.server.on("POST", NOTES, replies.networkError("offline"));
    const note = saveNote(LIST, "late", fresh.core).then(
      (value) => ({ value }),
      (error: unknown) => ({ error }),
    );
    await waitFor("the note to wait in memory", async () => (await storageStatus(fresh.core)).pending === 1);
    await sleep(500);
    expect(kv4.operations.filter((o) => o.op === "set" && o.key === Persisted.queueKey), "no set of the unreadable queue").toEqual([]);
    expect((await storageStatus(fresh.core)).pending).toBe(1);

    kv4.heal();
    emitLifecycle(fresh.core, "active");
    await waitFor("the queue to be readable again", async () => (await storageStatus(fresh.core)).queueReadable);
    await waitFor("the queue of one to be written", () =>
      kv4.operations.some((o) => o.op === "set" && o.key === Persisted.queueKey && o.failure === undefined && Persisted.queueCount(o.value) === 1),
    );

    const before = fresh.server.count("POST", NOTES);
    fresh.server.on("POST", NOTES, replies.text(201, "{}"));
    emitConnectivity(fresh.core, true, "wifi");
    expect(await note, "the note resolves after its replay").toEqual({ value: true });
    await waitFor("the queue to be empty", async () => (await storageStatus(fresh.core)).pending === 0);
    await sleep(200);
    const replays = fresh.server.requestsTo("POST", NOTES).slice(before);
    expect(replays.map((r) => r.text()), "the note replayed once").toEqual(["save:late"]);
    fresh.core.close();
    expect(fresh.core.closed).toBe(true);
  });

  await step("5. nothing panicked or trapped, and the core is the one loaded at the start", async () => {
    expect((await counters(core)).panics).toBe(panicsBefore);
    expect(core.closed).toBe(false);
    expect(await add(2, 3, core)).toBe(5);
  });

  handle?.close();
});
