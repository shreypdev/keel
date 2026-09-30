import { expect, test } from "vitest";
import { ALL_SIGNALS, type KeelCore, KeelError, KeelTransportError } from "@keel/runtime";
import { Todos, add, explode } from "@playground/core";
import { boot, bootRaw } from "../src/harness.js";
import { step, waitFor } from "../src/wait.js";
import { restore, snapshot } from "../src/wasm-exports.js";

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
function adoptTodos(core: KeelCore, handle: bigint): Todos {
  const construct = Todos as unknown as new (core: KeelCore, handle: bigint) => Todos;
  return new construct(core, handle);
}

test("S17 panic containment", async () => {
  // The bystander: a second core in this process, loaded before anything goes wrong.
  const bystander = await boot();
  const bystanderTodos = await Todos.create(bystander.core);
  await bystanderTodos.add("untouched");

  const victim = await bootRaw();
  const todos = await Todos.create(victim.core);
  await todos.add("a");
  const b = await todos.add("b");
  await todos.toggle(b.id);
  let saved: Uint8Array = new Uint8Array(0);

  await step("1. a snapshot of a core with two items", () => {
    saved = snapshot(victim.transport);
    expect(saved.length).toBeGreaterThan(0);
  });

  await step("2. explode('kaboom') fails the call, closes the core, and logs fatally first", async () => {
    const failure = await explode("kaboom", victim.core).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(failure, "the call fails instead of hanging").toBeInstanceOf(KeelError);
    expect(failure).toBeInstanceOf(KeelTransportError);
    expect((failure as KeelTransportError).reason).toBe("trap");
    expect((failure as KeelTransportError).message).toMatch(/trapped/);

    await waitFor("the core to close", () => victim.core.closed);
    expect(victim.closed, "onClose fired, once").toHaveLength(1);
    expect(victim.closed[0]).toBeInstanceOf(KeelTransportError);

    const fatal = victim.log.find({ minLevel: 5, contains: "kaboom" });
    expect(fatal, "a fatal record mentioning kaboom reached the Log port").toHaveLength(1);
    expect(fatal[0]?.target).toBe("keel::panic");

    // A closed core refuses everything, with a typed error.
    await expect(add(1, 2, victim.core)).rejects.toBeInstanceOf(KeelTransportError);
  });

  await step("3. the page can restart: a fresh load succeeds, and the snapshot restores into it", async () => {
    // The plain `KeelCore.load`, as an app's restart would do.
    const fresh = await boot();
    expect(await add(1, 2, fresh.core)).toBe(3);

    // And the recovery proper: a core that gets the snapshot back (this needs the transport, see bootRaw).
    const restarted = await bootRaw();
    restore(restarted.transport, saved);
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
});
