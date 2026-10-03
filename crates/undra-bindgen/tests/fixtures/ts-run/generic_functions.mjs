// Execution test of the `generic_functions` golden case (ADR-058): the instantiations of a generic function are an
// overload set that takes the name of its type first, each one calls the id of its own instantiation with the
// arguments of a hand-written function, and a call that bypassed the types fails as a refused call of its kind.
// Usage: node generic_functions.mjs <package>.

import assert from "node:assert/strict";

import { bytes, fakeCoreClass, setup } from "./lib.mjs";

const { rt, types, errors, objects, UndraIds } = await setup(process.argv[2]);
const { encodeValue, codecs, CallTarget, UndraCallError } = rt;
const FakeCore = fakeCoreClass(rt);
const core = new FakeCore();
const free = { target: CallTarget.FreeFunction };

// The literal picks the instantiation; the arguments are encoded as the instantiation's own types say.
const todo = { id: 3, title: "t" };
const note = { id: 4, body: "n" };
core.replies.push(encodeValue(codecs.option(types.TodoCodec), todo));
assert.deepEqual(await objects.newest("Todo", [todo], core), todo);
assert.deepEqual(core.calls.at(-1), {
  target: free,
  methodId: UndraIds.Functions.newestTodo,
  args: "01000000" + "03000000" + "0100000074",
  signal: undefined,
});
core.replies.push(encodeValue(codecs.option(types.NoteCodec), null));
assert.equal(await objects.newest("Note", [], core), null);
assert.equal(core.calls.at(-1).methodId, UndraIds.Functions.newestNote);
assert.notEqual(UndraIds.Functions.newestTodo, UndraIds.Functions.newestNote);

// A type parameter no argument fixes is the literal alone; an `async` one forwards the abort signal.
core.replies.push(encodeValue(types.NoteCodec, note));
assert.deepEqual(await objects.draft("Note", core), note);
assert.equal(core.calls.at(-1).methodId, UndraIds.Functions.draftNote);
assert.equal(core.calls.at(-1).args, "");
const controller = new AbortController();
core.replies.push(encodeValue(types.TodoCodec, todo));
assert.deepEqual(await objects.load("Todo", 9, core, controller.signal), todo);
assert.equal(core.calls.at(-1).methodId, UndraIds.Functions.loadTodo);
assert.equal(core.calls.at(-1).args, "09000000");
assert.equal(core.calls.at(-1).signal, controller.signal);

// The typed error of the family is the family's.
core.replies.push(new rt.UndraReplyError(rt.ReplyStatus.Error, encodeValue(errors.LoadErrorCodec, new errors.LoadError.Offline())));
await assert.rejects(objects.load("Note", 1, core), (e) => e instanceof errors.LoadError.Offline);

// A stream, a command and an `async` function that returns nothing.
core.streams.push([encodeValue(types.TodoCodec, todo)]);
const seen = [];
for await (const row of objects.rows("Todo", 1, core)) seen.push(row);
assert.deepEqual(seen, [todo]);
assert.equal(core.calls.at(-1).methodId, UndraIds.Functions.rowsTodo);
await objects.forget("Note", note, core);
assert.equal(core.calls.at(-1).methodId, UndraIds.Functions.forgetNote);
await objects.save("Todo", todo, core);
assert.equal(core.calls.at(-1).methodId, UndraIds.Functions.saveTodo);

// A parameter called `type` does not collide with the literal's.
core.replies.push(encodeValue(types.TodoCodec, todo));
assert.deepEqual(await objects.make("Todo", "kind", core), todo);
assert.equal(core.calls.at(-1).methodId, UndraIds.Functions.makeTodo);
assert.equal(core.calls.at(-1).args, "04000000" + "6b696e64");

// A function that is not generic stands beside the families.
core.replies.push(encodeValue(codecs.u32, 5));
assert.equal(await objects.countRows(core), 5);

// Methods of an object are an overload set too, and an instantiation is not callable under another's name.
const library = await objects.Library.create(core);
core.replies.push(encodeValue(codecs.vec(types.NoteCodec), [note]));
assert.deepEqual(await library.pinned("Note"), [note]);
assert.equal(core.calls.at(-1).methodId, UndraIds.Objects.Library.pinnedNote);
assert.deepEqual(core.calls.at(-1).target, { target: CallTarget.ObjectMethod, handle: 7n });
core.replies.push(encodeValue(codecs.u32, 2));
assert.equal(await library.remember("Todo", todo), 2);
assert.equal(core.calls.at(-1).methodId, UndraIds.Objects.Library.rememberTodo);

// A call that bypassed the types (plain JavaScript, a cast) is refused the way a refused call of its kind is:
// a rejected promise, a throw for a stream, a report for a command. Nothing reaches the core.
const before = core.calls.length;
await assert.rejects(objects.newest("Draft", [], core), (e) => e instanceof UndraCallError.Refused && /newest is not declared for Draft/.test(e.message));
await assert.rejects(library.pinned("Draft"), (e) => e instanceof UndraCallError.Refused);
assert.throws(() => objects.rows("Draft", 1, core), (e) => e instanceof UndraCallError.Refused);
await objects.forget("Draft", note, core);
assert.equal(core.reports.at(-1).operation, "forget");
assert.ok(core.reports.at(-1).error instanceof UndraCallError.Refused);
assert.equal(core.calls.length, before);

console.log("ok");
