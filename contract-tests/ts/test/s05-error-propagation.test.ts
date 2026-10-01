import { expect, test } from "vitest";
import { CallTarget, UndraReplyError, ReplyStatus } from "@undra/runtime";
import { BigList, UndraIds, LabError, ListError, TodoError, Todos, add, failLater, parseCount } from "@playground/core";
import { boot } from "../src/harness.js";
import { counters } from "../src/stats.js";
import { step } from "../src/wait.js";

// S05 error propagation: a typed error arrives as its typed class on the same path as a success
// (the promise rejects), a bad request arrives as a reply with status 5 and a reason, and a
// failure never changes the core's state or harms it.

const NO_ARGS = new Uint8Array(0);

/** What `run` rejected with; the scenario fails if it resolved. */
async function failure(run: () => Promise<unknown>): Promise<unknown> {
  try {
    await run();
  } catch (error) {
    return error;
  }
  throw new Error("expected the call to fail, but it succeeded");
}

/** Expects `error` to be a bad-request reply that says why. */
function expectBadRequest(error: unknown): void {
  expect(error).toBeInstanceOf(UndraReplyError);
  expect((error as UndraReplyError).status).toBe(ReplyStatus.BadRequest);
  expect((error as UndraReplyError).reason, "the bad request carries a reason").toBeTruthy();
}

test("S05 error propagation", async () => {
  const { core } = await boot();
  const start = await counters(core);

  await step("1. a sync typed error is delivered on the failure path", async () => {
    expect(await failure(() => parseCount("x", core))).toMatchObject({ kind: "notANumber", value: "x" });
    await expect(parseCount("x", core)).rejects.toBeInstanceOf(LabError.NotANumber);
    // The same error through the synchronous entry point: a throw of the reply error, carrying the typed error's bytes.
    const w = new TextEncoder().encode("x");
    const args = new Uint8Array([1, 0, 0, 0, ...w]);
    let thrown: unknown;
    try {
      core.callSync(CallTarget.FreeFunction, UndraIds.Functions.parseCount, args);
    } catch (error) {
      thrown = error;
    }
    expect(thrown).toBeInstanceOf(UndraReplyError);
    expect((thrown as UndraReplyError).status).toBe(ReplyStatus.Error);
    expect(LabError.fromReply(thrown)).toBeInstanceOf(LabError.NotANumber);
  });

  await step("2. an async typed error", async () => {
    const error = await failure(() => failLater(10, 7, core));
    expect(error).toBeInstanceOf(LabError.Rejected);
    expect(error).toMatchObject({ code: 7, reason: "on purpose" });
  });

  await step("3. store errors change nothing", async () => {
    const todos = await Todos.create(core);
    const before = await counters(core);
    expect(await failure(() => todos.add("   "))).toBeInstanceOf(TodoError.EmptyTitle);
    const after = await counters(core);
    expect(after.transactions - before.transactions, "a refused add delivers no change-set").toBe(0);
    expect(todos.todos.peek()).toEqual([]);

    const list = await BigList.create(core);
    const tooFar = await failure(() => list.removeAt(10_000));
    expect(tooFar).toBeInstanceOf(ListError.OutOfRange);
    expect(tooFar).toMatchObject({ index: 10_000, len: 10_000 });

    const past = await failure(() => list.insertAt(10_001, "x"));
    expect(past).toBeInstanceOf(ListError.OutOfRange);
    expect(past).toMatchObject({ index: 10_001, len: 10_000 });
    // The refused insert did not use up an identity.
    expect(await list.insertAt(0, "y")).toBe(10_001);
    expect(list.items.peek()[0]).toEqual({ id: 10_001, label: "y", version: 0 });
    expect(list.count.peek()).toBe(10_001);
    todos.close();
    list.close();
  });

  await step("4. reply statuses", async () => {
    // An unknown method id (a free function nobody defined).
    expectBadRequest(await failure(() => core.call(CallTarget.FreeFunction, 0xdead_beef, NO_ARGS)));

    // A method of a handle that was released.
    const todos = await Todos.create(core);
    const released = todos.handle;
    todos.close();
    expectBadRequest(
      await failure(() => core.call({ target: CallTarget.ObjectMethod, handle: released }, UndraIds.Objects.Todos.clearDone, NO_ARGS)),
    );

    // A constructor given arguments it cannot decode: RemoteTodosQueryHandle needs a list name; it gets nothing.
    expectBadRequest(
      await failure(() => core.construct(UndraIds.Objects.RemoteTodosQueryHandle.typeId, UndraIds.Objects.RemoteTodosQueryHandle.new, NO_ARGS)),
    );
  });

  await step("5. the core still works, and counted exactly the three bad requests", async () => {
    expect(await add(1, 2, core)).toBe(3);
    const end = await counters(core);
    expect(end.badRequests - start.badRequests).toBe(3);
  });
});
