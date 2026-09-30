import { expect, test } from "vitest";
import { RemoteError, RemoteTodosQueryHandle, configureRemote } from "@playground/core";
import { BASE_URL, boot } from "../src/harness.js";
import { replies } from "../src/fake-server.js";
import { CLOCK_START_MS } from "../src/manual-clock.js";
import { counters } from "../src/stats.js";
import { step, waitFor } from "../src/wait.js";

// S12 query: fetch, stale, refetch. A query is fetched when first observed, shared by every
// observer of the same list, served from the cache while fresh (30 s), refetched when stale, on
// request, and on invalidation; a failing server ends in a typed error while the last good data
// stays.

const LIST = "s12";
const URL = `${BASE_URL}/lists/${LIST}/todos`;
const milk = { id: 1, title: "Buy milk", done: false };
const dog = { id: 2, title: "Walk the dog", done: false };

test("S12 query: fetch, stale, refetch", async () => {
  const { core, server, clock } = await boot();
  await configureRemote({ baseUrl: BASE_URL }, core);
  server.on("GET", URL, replies.json(200, [milk]));
  const gets = () => server.count("GET", URL);
  const handlesBefore = (await counters(core)).liveHandles;

  const first = await RemoteTodosQueryHandle.create(LIST, core);
  const statuses: string[] = [first.status.peek()];
  first.status.subscribe((s) => statuses.push(s));

  await step("1. first observer: fetching, then success with the server's data", async () => {
    expect(first.status.peek(), "a fetch is pending right away").toBe("fetching");
    await waitFor("the first fetch to succeed", () => first.status.peek() === "success");
    expect(statuses, "fetching, then success, nothing in between").toEqual(["fetching", "success"]);
    expect(first.data.peek()).toEqual([milk]);
    expect(first.error.peek()).toBeNull();
    expect(first.fetching.peek()).toBe(false);
    expect(first.updatedAt.peek()).toBe(CLOCK_START_MS);
    expect(gets()).toBe(1);
  });

  clock.advance(10_000);
  const second = await RemoteTodosQueryHandle.create(LIST, core);

  await step("2. ten seconds later a second observer gets the cached data at once, without a request", async () => {
    // Nothing awaited since create() returned: the data came with the initial change-set.
    expect(second.data.peek()).toEqual([milk]);
    expect(gets(), "a fresh entry is not fetched again").toBe(1);
    expect(second.data.peek()).toEqual(first.data.peek());
  });

  let third!: RemoteTodosQueryHandle;
  await step("3. 41 s after the fetch the data is stale: a new observer fetches, and everybody sees the result", async () => {
    clock.advance(31_000);
    server.on("GET", URL, replies.json(200, [milk, dog]));
    third = await RemoteTodosQueryHandle.create(LIST, core);
    expect(gets(), "the stale entry is fetched by the new observer").toBe(2);
    const refreshed = CLOCK_START_MS + 41_000;
    for (const [name, handle] of [["first", first], ["second", second], ["third", third]] as const) {
      await waitFor(`the ${name} handle to show two items`, () => handle.data.peek()?.length === 2 && handle.status.peek() === "success");
      expect(handle.data.peek()).toEqual([milk, dog]);
      expect(handle.updatedAt.peek(), `${name} handle's updatedAt`).toBe(refreshed);
    }
    expect(first.updatedAt.peek()).toBeGreaterThan(CLOCK_START_MS);
  });

  await step("4. refetch() fetches although the data is fresh", async () => {
    await second.refetch();
    expect(gets()).toBe(3);
    await waitFor("the refetch to finish", () => second.fetching.peek() === false && second.status.peek() === "success");
  });

  await step("5. invalidate() marks the entry stale and, being observed, refetches", async () => {
    await third.invalidate();
    await waitFor("the invalidation to refetch", () => gets() === 4);
    await waitFor("the refetch to finish", () => third.fetching.peek() === false && third.status.peek() === "success");
  });

  await step("6. a 503: after the retry the status is error, with the last good data still there", async () => {
    server.on("GET", URL, replies.text(503, "down"));
    void first.refetch();
    await waitFor("status error", () => first.status.peek() === "error");
    expect(first.error.peek()).toBeInstanceOf(RemoteError.Status);
    expect(first.error.peek()).toMatchObject({ kind: "status", code: 503 });
    expect(first.data.peek(), "the data is the last good list").toEqual([milk, dog]);
    expect(first.fetching.peek()).toBe(false);
    // One retry: the first attempt and its retry (scenarios.md: "1 retry, about 1 s of backoff").
    expect(gets()).toBe(6);
  });

  await step("7. a body that is not JSON ends in BadBody", async () => {
    server.on("GET", URL, replies.text(200, "not json"));
    void first.refetch();
    await waitFor("a BadBody error", () => first.error.peek() instanceof RemoteError.BadBody && first.status.peek() === "error");
    expect(first.data.peek()).toEqual([milk, dog]);
  });

  await step("8. releasing every handle frees them all", async () => {
    first.close();
    second.close();
    third.close();
    await waitFor("live_handles to return to its earlier value", async () => (await counters(core)).liveHandles === handlesBefore);
  });
});
