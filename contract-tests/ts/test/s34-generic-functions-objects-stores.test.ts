import { expect, test } from "vitest";
import { CallTarget, ChangeOp, type Codec, type PatchOp, ReplyStatus, UndraReader, UndraReplyError, decodePatch, fnv1a32 } from "@undra/runtime";
import {
  type Note,
  NoteCodec,
  NoteSelection,
  RecentTodos,
  type Todo,
  TodoCodec,
  TodoSelection,
  UndraIds,
  draft,
  newest,
  recentTodos,
} from "@playground/core";
import { boot } from "../src/harness.js";
import { RawStore, type SignalUpdate } from "../src/raw-store.js";
import { step } from "../src/wait.js";

// S34 generic functions, objects and stores (ADR-058): the playground's selection module through the generated API only
// (the raw API where a step says so). One generic function is a function per listed type, one generic store is a store per
// alias with its own type id, handle and signals, and a generic object is a class of the alias's name.

/** A to-do whose identity carries the counter `n` (the first eight bytes, big-endian: what the core orders by). */
const todo = (n: number, title: string): Todo => ({ id: `00000000-0000-${n.toString(16).padStart(4, "0")}-0000-000000000000`, title, done: false });

/** A note whose id is the counter `n`. */
const note = (n: number, title: string): Note => ({ id: BigInt(n), title, done: false });

/** The keyed-patch operations of the `rows` entry (signal 0) of a change-set's entries. */
function rowOps<T>(entries: readonly SignalUpdate[], codec: Codec<T>): PatchOp<T>[] {
  const found = entries.filter((e) => e.signalId === 0);
  expect(found, "one entry for the rows signal").toHaveLength(1);
  const entry = found[0] as SignalUpdate;
  expect(entry.op, "the entry is a keyed patch").toBe(ChangeOp.KeyedPatch);
  const r = new UndraReader(entry.value);
  const ops = decodePatch(r, codec);
  r.finish();
  return ops;
}

test("S34 generic functions, objects and stores", async () => {
  const { core, runtimeErrors } = await boot();

  await step("1. newest of three to-dos and of two notes is the right row of the right type, of none is nothing", async () => {
    const todos = [todo(5, "five"), todo(9, "nine"), todo(7, "seven")];
    const notes = [note(2, "two"), note(1, "one")];
    expect(await newest("Todo", todos, core)).toEqual(todo(9, "nine"));
    expect(await newest("Note", notes, core)).toEqual(note(2, "two"));
    expect(await newest("Todo", [], core)).toBeNull();
    expect(await newest("Note", [], core)).toBeNull();
  });

  await step("2. draft names its type and returns a row of it with a fresh id", async () => {
    const first = await draft("Todo", "first", core);
    const second = await draft("Todo", "second", core);
    expect(first.title).toBe("first");
    expect(first.done).toBe(false);
    expect(second.id, "every draft has an identity of its own").not.toBe(first.id);
    const aNote = await draft("Note", "a note", core);
    expect(aNote.title).toBe("a note");
    expect(typeof aNote.id).toBe("bigint");
    // A drafted row is an ordinary row: the generic function `newest` takes it.
    expect((await newest("Todo", [first, second], core))?.title).toBe("second");
  });

  await step("3. TodoSelection and NoteSelection are two stores: one keyed Insert to the one that was ticked, nothing to the other", async () => {
    const todos = await RawStore.open(core, UndraIds.Objects.TodoSelection);
    const notes = await RawStore.open(core, UndraIds.Objects.NoteSelection);
    expect(todos.handle).not.toBe(notes.handle);
    expect(UndraIds.Objects.TodoSelection.typeId).not.toBe(UndraIds.Objects.NoteSelection.typeId);
    todos.take();
    notes.take();
    const toggleTodo = (row: Todo) => todos.callWith(UndraIds.Objects.TodoSelection.toggle, TodoCodec, row);

    await toggleTodo(todo(3, "three"));
    let entries = todos.take();
    expect(rowOps(entries, TodoCodec)).toEqual([{ op: "insert", index: 0, item: todo(3, "three") }]);
    // `count` follows, in the same change-set (signal 1, a computed `u32`).
    expect(entries.map((e) => e.signalId).sort()).toEqual([0, 1]);
    expect(notes.take(), "the note selection heard nothing").toEqual([]);

    await toggleTodo(todo(3, "three"));
    entries = todos.take();
    expect(rowOps(entries, TodoCodec)).toEqual([{ op: "remove", index: 0 }]);
    expect(notes.take()).toEqual([]);

    await notes.callWith(UndraIds.Objects.NoteSelection.toggle, NoteCodec, note(4, "four"));
    expect(rowOps(notes.take(), NoteCodec)).toEqual([{ op: "insert", index: 0, item: note(4, "four") }]);
    expect(todos.take(), "the to-do selection heard nothing").toEqual([]);
    todos.close();
    notes.close();
  });

  await step("4. snapshot and restore: both selections keep their rows and handles", async () => {
    const todos = await TodoSelection.create(core);
    const notes = await NoteSelection.create(core);
    await todos.toggle(todo(1, "one"));
    await todos.toggle(todo(2, "two"));
    await notes.toggle(note(8, "eight"));
    expect(todos.count.peek()).toBe(2);
    expect(notes.count.peek()).toBe(1);
    const handles = [todos.handle, notes.handle];
    const snapshot = await core.snapshot();

    await todos.clear();
    await notes.toggle(note(9, "nine"));
    expect(todos.rows.peek()).toEqual([]);

    await core.restore(snapshot);
    expect([todos.handle, notes.handle]).toEqual(handles);
    expect(todos.rows.peek()).toEqual([todo(1, "one"), todo(2, "two")]);
    expect(todos.count.peek()).toBe(2);
    expect(notes.rows.peek()).toEqual([note(8, "eight")]);
    expect(notes.count.peek()).toBe(1);
    // The restored stores are the instantiations' own: they keep working, each on its own rows.
    await todos.toggle(todo(1, "one"));
    expect(todos.rows.peek()).toEqual([todo(2, "two")]);
    expect(notes.rows.peek()).toEqual([note(8, "eight")]);
    todos.close();
    notes.close();
  });

  await step("5. an id that names no instantiation is refused as a bad request, and the core is unharmed", async () => {
    const refused = await core.call({ target: CallTarget.FreeFunction }, fnv1a32("fn.newest<Draft>"), new Uint8Array(4)).catch((e: unknown) => e);
    expect(refused).toBeInstanceOf(UndraReplyError);
    expect((refused as UndraReplyError).status).toBe(ReplyStatus.BadRequest);
    // The ids of the instantiations that exist are the ones the bindings use.
    expect(UndraIds.Functions.newestTodo).toBe(fnv1a32("fn.newest<Todo>"));
    expect(UndraIds.Functions.newestNote).toBe(fnv1a32("fn.newest<Note>"));
    expect((await newest("Todo", [todo(1, "one")], core))?.title).toBe("one");
  });

  await step("6. a RecentTodos a function returns is a class of the alias's name, one wrapper per object", async () => {
    const recent = await recentTodos([todo(1, "one"), todo(2, "two"), todo(3, "three")], 2, core);
    expect(recent).toBeInstanceOf(RecentTodos);
    expect(await recent.rows()).toEqual([todo(3, "three"), todo(2, "two")]);
    expect(await recent.latest()).toEqual(todo(3, "three"));
    await recent.open(todo(2, "two"));
    expect(await recent.rows()).toEqual([todo(2, "two"), todo(3, "three")]);
    const other = await recentTodos([], 5, core);
    expect(other).not.toBe(recent);
    expect(await other.latest(), "a second object has rows of its own").toBeNull();
    other.close();
    expect(await recent.latest(), "closing one object leaves the other").toEqual(todo(2, "two"));
    recent.close();
  });

  expect(runtimeErrors, "nothing was reported without a caller").toEqual([]);
});
