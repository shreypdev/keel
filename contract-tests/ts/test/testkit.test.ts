import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { afterEach, expect, test } from "vitest";
import { RemoteTodosQueryHandle, Todos, UndraIds, configureRemote, fileRead, fileWrite, kvGet, kvPut, secretGet } from "@playground/core";
import { PreviewCore, parseSeed, response } from "@undra/testkit";
import { PLAYGROUND_WASM } from "../src/harness.js";

// The TypeScript testing kit (docs/TESTING.md) against the real playground core: PreviewCore loads the app's own wasm core with the
// deterministic fakes as its ports and a manual clock. Not a numbered scenario (the grid is the platforms agreeing on the boundary);
// this is the kit's own proof that the fakes and the real core work together.

const seedText = await readFile(fileURLToPath(new URL("../../../testkit/fixtures/seed.json", import.meta.url)), "utf8");
const wasm = await readFile(PLAYGROUND_WASM);

const previews: PreviewCore[] = [];
afterEach(() => {
  for (const p of previews.splice(0)) p.close();
});
const load = async (seed: string | undefined = seedText): Promise<PreviewCore> => {
  const preview = await PreviewCore.load({ wasm: new Uint8Array(wasm), expectedSchemaHash: UndraIds.schemaHash, shared: false, ...(seed !== undefined && { seed }) });
  previews.push(preview);
  return preview;
};

test("T1 preview: a store runs the real logic on the fakes", async () => {
  const preview = await load();
  const todos = await Todos.create(preview.core);
  await todos.add("Buy milk");
  const walk = await todos.add("Walk the dog");
  await todos.toggle(walk.id);
  await preview.settle();
  expect(todos.visible.peek().map((t) => t.title)).toEqual(["Buy milk", "Walk the dog"]);
  expect(todos.remaining.peek()).toBe(1);
});

test("T2 preview: the seed's HTTP rules answer the core's query, and the manual clock makes it stale", async () => {
  const preview = await load();
  await configureRemote({ baseUrl: "https://api.test" }, preview.core);
  const first = await RemoteTodosQueryHandle.create("inbox", preview.core);
  await preview.settle();
  expect(first.status.peek()).toBe("success");
  expect(first.data.peek()?.map((t) => t.title)).toEqual(["Buy milk", "Walk the dog"]);
  expect(first.updatedAt.peek()).toBe(1_700_000_000_000 /* the seed's now_ms */);
  expect(preview.fakes.http.calls.map((r) => r.url)).toEqual(["https://api.test/lists/inbox/todos"]);

  // Fresh for 30 s: a second observer is served from the cache.
  await preview.advance(10_000);
  await RemoteTodosQueryHandle.create("inbox", preview.core);
  expect(preview.fakes.http.calls).toHaveLength(1);

  // Past it, the next observer fetches again, and everyone sees the new list.
  preview.fakes.http.reset();
  preview.fakes.http.respond("https://api.test/lists/inbox/todos", response(200, JSON.stringify([{ id: 3, title: "Third", done: false }])));
  await preview.advance(31_000);
  await RemoteTodosQueryHandle.create("inbox", preview.core);
  await preview.settle();
  expect(preview.fakes.http.calls).toHaveLength(1);
  expect(first.data.peek()?.map((t) => t.title)).toEqual(["Third"]);
  expect(first.updatedAt.peek()).toBe(1_700_000_041_000);
  expect(preview.clock.nowMs()).toBe(1_700_000_041_000);
});

test("T3 preview: the kv fake is the query cache's persistence, and a seeded kv is read back", async () => {
  const preview = await load(JSON.stringify({ now_ms: 5_000 }));
  await configureRemote({ baseUrl: "https://api.test" }, preview.core);
  preview.fakes.http.respond("https://api.test/lists/inbox/todos", response(200, "[]"));
  await RemoteTodosQueryHandle.create("inbox", preview.core);
  // The query is persisted (`persist`): the core writes its cache through the Kv port, a moment after the fetch (it waits on a timer,
  // so it is the manual clock that lets it happen).
  await preview.advance(5_000);
  expect(preview.fakes.kv.ops.map((op) => op.op)).toContain("set");
  expect(preview.fakes.kv.keys().length).toBeGreaterThan(0);
});

test("T5 preview: the seeded ports are what the core reads, and its writes land in the fakes", async () => {
  const preview = await load();
  const text = (bytes: Uint8Array | null | undefined): string | undefined => (bytes === null || bytes === undefined ? undefined : new TextDecoder().decode(bytes));
  expect(text(await kvGet("greeting", preview.core))).toBe("hello");
  expect(await kvGet("absent", preview.core)).toBeNull();
  await kvPut("saved", Uint8Array.of(1, 2, 3), preview.core);
  expect(preview.fakes.kv.value("saved")).toEqual(Uint8Array.of(1, 2, 3));
  expect(text(await secretGet("token", preview.core))).toBe("t-123");
  expect(text(await fileRead("notes/a.txt", preview.core))).toBe("hello");
  await fileWrite("out/b.txt", new TextEncoder().encode("written"), preview.core);
  expect(text(preview.fakes.fs.contents("out/b.txt"))).toBe("written");
  await expect(fileRead("nope", preview.core)).rejects.toMatchObject({ kind: "notFound" });
});

test("T4 preview: the same seed document reads in every kit", () => {
  const seed = parseSeed(seedText);
  expect(seed.http).toHaveLength(2);
  expect(seed.nowMs).toBe(1_700_000_000_000);
});
