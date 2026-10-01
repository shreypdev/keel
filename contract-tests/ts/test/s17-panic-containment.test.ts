import { expect, test } from "vitest";
import { ALL_SIGNALS, UndraCallError, type UndraCore, UndraError, UndraTransportError, UndraUnhandledError } from "@undra/runtime";
import { Counter, RemoteTodosQueryHandle, Todos, add, addLater, configureRemote, createRemoteTodo, explode, parseCount } from "@playground/core";
import { BASE_URL, boot, bootWorker } from "../src/harness.js";
import { replies } from "../src/fake-server.js";
import { step, waitFor } from "../src/wait.js";

// S17 panic containment (the wasm variant): the shipped wasm profile aborts on a panic (SPEC 7),
// so containment means the host survives and recovers: the failing call rejects, the core is
// closed, the fatal record reached the Log port first, a new core restores the snapshot, and a
// second core in the same process never noticed.

/**
 * A `Todos` over a handle that already exists in the core (one a restore brought back). The
 * generated class has no public way to adopt a handle, only `create()`, which constructs a new
 * one: see NOTES.md ("no way to adopt a restored handle"). The constructor is private in the
 * type system only, so the scenario reaches it the way the generated `create()` does.
 */
function adoptTodos(core: UndraCore, handle: bigint): Todos {
  const construct = Todos as unknown as new (core: UndraCore, handle: bigint) => Todos;
  return new construct(core, handle);
}

test("S17 panic containment", async () => {
  // The bystander: a second core in this process, loaded before anything goes wrong.
  const bystander = await boot();
  const bystanderTodos = await Todos.create(bystander.core);
  await bystanderTodos.add("untouched");

  const victim = await boot();
  const victimCounter = await Counter.create(victim.core);
  const todos = await Todos.create(victim.core);
  await todos.add("a");
  const b = await todos.add("b");
  await todos.toggle(b.id);
  let saved: Uint8Array = new Uint8Array(0);

  await step("1. a snapshot of a core with two items", async () => {
    saved = await victim.core.snapshot();
    expect(saved.length).toBeGreaterThan(0);
  });

  await step("2. explode('kaboom') fails the call, closes the core, and logs fatally first", async () => {
    const failure = await explode("kaboom", victim.core).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(failure, "the call fails instead of hanging").toBeInstanceOf(UndraError);
    expect(failure, "the generated call maps the trap onto the closed set").toBeInstanceOf(UndraCallError.Unavailable);
    const transport = (failure as UndraCallError.Unavailable).transport;
    expect(transport.reason).toBe("trap");
    expect(transport.message).toMatch(/trapped/);

    await waitFor("the core to close", () => victim.core.closed);
    expect(victim.closed, "onClose fired, once").toHaveLength(1);
    expect(victim.closed[0]).toBeInstanceOf(UndraTransportError);

    const fatal = victim.log.find({ minLevel: 5, contains: "kaboom" });
    expect(fatal, "a fatal record mentioning kaboom reached the Log port").toHaveLength(1);
    expect(fatal[0]?.target).toBe("undra::panic");

    // A closed core refuses everything, with a typed error.
    await expect(add(1, 2, victim.core)).rejects.toBeInstanceOf(UndraCallError.Unavailable);
  });

  await step("3. the page can restart: a fresh load succeeds, and the snapshot restores into it", async () => {
    // The plain `UndraCore.load`, as an app's restart would do.
    const fresh = await boot();
    expect(await add(1, 2, fresh.core)).toBe(3);

    // And the recovery proper: a core that gets the snapshot back.
    const restarted = await boot();
    await restarted.core.restore(saved);
    const revived = adoptTodos(restarted.core, todos.handle);
    await restarted.core.observe(revived.handle, ALL_SIGNALS, true);
    expect(revived.todos.peek().map((t) => [t.title, t.done])).toEqual([
      ["a", false],
      ["b", true],
    ]);
    expect(revived.remaining.peek()).toBe(1);
    // It is a live store again.
    const c = await revived.add("c");
    expect(c.id > b.id).toBe(true);
    expect(revived.todos.peek().map((t) => t.title)).toEqual(["a", "b", "c"]);
    revived.close();
  });

  await step("4. the core that was loaded before the panic was not affected", async () => {
    expect(bystander.core.closed).toBe(false);
    expect(bystander.closed).toEqual([]);
    expect(bystander.log.find({ minLevel: 4 })).toEqual([]);
    expect(bystanderTodos.todos.peek().map((t) => t.title)).toEqual(["untouched"]);
    await bystanderTodos.add("still working");
    expect(bystanderTodos.todos.peek().map((t) => t.title)).toEqual(["untouched", "still working"]);
    expect(await add(20, 22, bystander.core)).toBe(42);
  });

  await step("5. on the trapped core, calls through the generated bindings reject and none hangs", async () => {
    // An async call and a typed one reject with `Unavailable` (its transport reason is "trap" or "closed"); a store command
    // resolves and reports the same to `onError` (ADR-032, amendment A).
    const within = (what: string, call: Promise<unknown>): Promise<unknown> =>
      Promise.race([
        call,
        new Promise<unknown>((_, reject) => {
          setTimeout(() => reject(new Error(`${what} did not settle within a second`)), 1_000);
        }),
      ]);
    for (const [what, call] of [
      ["add_later", () => addLater(1, 1, 10, victim.core)],
      ["parse_count", () => parseCount("1", victim.core)],
    ] as const) {
      const failure = await within(what, call()).then(
        () => undefined,
        (e: unknown) => e,
      );
      expect(failure, `${what} on the trapped core rejects with Unavailable`).toBeInstanceOf(UndraCallError.Unavailable);
      expect(["trap", "closed"], `${what}: ${String(failure)}`).toContain((failure as UndraCallError.Unavailable).transport.reason);
    }
    victim.runtimeErrors.length = 0;
    await within("Counter.increment", victimCounter.increment());
    const reports = victim.runtimeErrors.splice(0) as UndraUnhandledError[];
    expect(reports.map((r) => r.operation)).toEqual(["Counter.increment"]);
    expect(reports[0]).toBeInstanceOf(UndraUnhandledError);
    expect(reports[0]?.error, "the command was reported as unavailable").toBeInstanceOf(UndraCallError.Unavailable);
    expect(["trap", "closed"]).toContain((reports[0]?.error as UndraCallError.Unavailable).transport.reason);
  });

  await step("6. worker mode: the core calls Clock, Rng and Log from inside the worker and does not trap", async () => {
    // The core runs behind a worker (`mode: "wasm-worker"`; the worker is served in this thread over a MessageChannel). It cannot
    // wait for the main thread, so the worker answers the three synchronous standard ports itself; the Http and Kv ports are
    // asynchronous and cross to the harness adapters. A worker that answered every port call "async" made the first clock read
    // panic (E0062), which on wasm is a trap that kills the core: a query reads the Clock on every observe.
    const list = "s17-worker";
    const url = `${BASE_URL}/lists/${list}/todos`;
    const milk = { id: 1, title: "Buy milk", done: false };
    const walk = { id: 2, title: "Walk", done: false };
    const worker = await bootWorker();
    expect(worker.core.mode).toBe("wasm-worker");
    await configureRemote({ baseUrl: BASE_URL }, worker.core);
    worker.server.on("GET", url, replies.json(200, [milk]));
    worker.server.on("POST", url, replies.json(201, walk));

    // The query reads the Clock when it is observed (and again to stamp the fetch).
    const query = await RemoteTodosQueryHandle.create(list, worker.core);
    await waitFor("the query to fetch through the Http port", () => query.status.peek() === "success");
    expect(query.data.peek()).toEqual([milk]);
    expect(query.updatedAt.peek(), "the fetch was stamped with the worker's clock").toBeGreaterThan(1_700_000_000_000);
    expect(worker.server.count("GET", url)).toBe(1);

    // The mutation draws its idempotency key from the Rng.
    worker.server.on("GET", url, replies.json(200, [milk, walk]));
    expect(await createRemoteTodo(list, "Walk", worker.core)).toEqual(walk);
    const post = worker.server.requestsTo("POST", url).at(-1);
    expect(post?.header("Idempotency-Key"), "the POST carried a key").toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
    await waitFor("the refetch to show the created item", () => query.data.peek()?.length === 2);

    // The core's own log records reach the harness Log adapter: an observation of a handle the core never issued is a warning
    // of the runtime (the promise stays pending until the core is closed after the scenario).
    void worker.core.observe(0x7777_7777_0000_0001n, ALL_SIGNALS, true).catch(() => {});
    await waitFor("the runtime's warning to reach the Log adapter", () => worker.log.find({ minLevel: 3, target: "undra::runtime" }).length > 0);

    expect(worker.core.closed, "the core is alive").toBe(false);
    expect(worker.closed, "onClose never fired").toEqual([]);
    expect(worker.log.find({ minLevel: 5 }), "nothing fatal was logged").toEqual([]);
    expect(await add(20, 22, worker.core)).toBe(42);
    query.close();
  });
});
