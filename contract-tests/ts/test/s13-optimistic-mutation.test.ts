import { expect, test } from "vitest";
import { type RemoteTodo, RemoteError, RemoteTodosQueryHandle, configureRemote, createRemoteTodo, setRemoteDone } from "@playground/core";
import { BASE_URL, boot } from "../src/harness.js";
import { replies } from "../src/fake-server.js";
import { step, waitFor } from "../src/wait.js";

// S13 optimistic mutation and rollback: a mutation shows its result in the cached list at once, is
// rolled back in one transaction if the server refuses, and is followed by a refetch if it succeeds.

const LIST = "s13";
const URL = `${BASE_URL}/lists/${LIST}/todos`;
const PATCH_URL = `${URL}/1`;
const DELAY_MS = 50;
/** Identity of the first optimistic placeholder: `u32::MAX`. */
const FIRST_PLACEHOLDER = 4_294_967_295;

const milk: RemoteTodo = { id: 1, title: "Buy milk", done: false };
const walk2: RemoteTodo = { id: 2, title: "Walk", done: false };
const isPlaceholder = (todo: RemoteTodo): boolean => todo.id >= 0x8000_0000;

/** Runs `run` and returns what it rejected with; the scenario fails if it resolved. */
async function failure(run: () => Promise<unknown>): Promise<unknown> {
  try {
    await run();
  } catch (error) {
    return error;
  }
  throw new Error("expected the call to fail, but it succeeded");
}

test("S13 optimistic mutation and rollback", async () => {
  const { core, server } = await boot();
  await configureRemote({ baseUrl: BASE_URL }, core);
  server.on("GET", URL, replies.json(200, [milk]));
  const handle = await RemoteTodosQueryHandle.create(LIST, core);

  // Every value `data` takes, in order (a value that arrives twice in a row is one value; the list that
  // was not fetched yet, `null`, is not a list).
  const recorded: RemoteTodo[][] = [];
  const record = (data: RemoteTodo[] | null): void => {
    if (data === null) return;
    const last = recorded[recorded.length - 1];
    if (last === undefined || JSON.stringify(last) !== JSON.stringify(data)) recorded.push(data);
  };
  record(handle.data.peek());
  handle.data.subscribe(record);
  await waitFor("the first list", () => recorded.length === 1);
  expect(recorded).toEqual([[milk]]);

  await step("1. POST 500: the placeholder shows, then the list is rolled back", async () => {
    server.on("POST", URL, replies.json(500, { error: "no" }, { delayMs: DELAY_MS }));
    const error = await failure(() => createRemoteTodo(LIST, "Walk", core));
    expect(error).toBeInstanceOf(RemoteError.Status);
    expect(error).toMatchObject({ kind: "status", code: 500 });
    await waitFor("the rollback", () => recorded.length === 3);
    const placeholder = { id: FIRST_PLACEHOLDER, title: "Walk", done: false };
    expect(recorded).toEqual([[milk], [milk, placeholder], [milk]]);
    expect(handle.status.peek()).toBe("success");
    expect(handle.data.peek()).toEqual([milk]);
  });

  await step("2. POST 201: the placeholder, then the server's item after the refetch; the request carries an Idempotency-Key", async () => {
    server.on("POST", URL, replies.json(201, { id: 2, title: "Walk", done: false }, { delayMs: DELAY_MS }));
    server.on("GET", URL, replies.json(200, [milk, walk2]));
    const created = await createRemoteTodo(LIST, "Walk", core);
    expect(created).toEqual(walk2);
    await waitFor("the refetched list", () => recorded.length === 5);
    expect(recorded.slice(3).map((list) => list.map((todo) => (isPlaceholder(todo) ? "placeholder" : todo.title)))).toEqual([
      ["Buy milk", "placeholder"],
      ["Buy milk", "Walk"],
    ]);
    expect(recorded[4]).toEqual([milk, walk2]);
    const posts = server.requestsTo("POST", URL);
    const key = posts[posts.length - 1]?.header("Idempotency-Key");
    expect(key, "the POST carried an Idempotency-Key").toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
    expect(posts[posts.length - 1]?.json()).toEqual({ title: "Walk" });
  });

  await step("3. PATCH 500: done shows, then is rolled back", async () => {
    server.on("PATCH", PATCH_URL, replies.json(500, { error: "no" }, { delayMs: DELAY_MS }));
    const error = await failure(() => setRemoteDone(LIST, 1, true, core));
    expect(error).toBeInstanceOf(RemoteError.Status);
    expect(error).toMatchObject({ code: 500 });
    await waitFor("the rollback", () => recorded.length === 7);
    expect(recorded.slice(5)).toEqual([
      [{ ...milk, done: true }, walk2],
      [milk, walk2],
    ]);
  });

  await step("4. PATCH 200: the list has done = true after the refetch", async () => {
    const done = { ...milk, done: true };
    server.on("PATCH", PATCH_URL, replies.json(200, done, { delayMs: DELAY_MS }));
    server.on("GET", URL, replies.json(200, [done, walk2]));
    const getsBefore = server.count("GET", URL);
    expect(await setRemoteDone(LIST, 1, true, core)).toEqual(done);
    await waitFor("the refetch after the success", () => server.count("GET", URL) > getsBefore);
    await waitFor("the query to settle", () => handle.status.peek() === "success" && handle.fetching.peek() === false);
    expect(handle.data.peek()).toEqual([done, walk2]);
    expect(server.requestsTo("PATCH", PATCH_URL).map((r) => r.json())).toEqual([{ done: true }, { done: true }]);
  });

  handle.close();
});
