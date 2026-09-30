// Execution test of the `stores` golden case: every signal type decodes from a change-set entry.
// Usage: node stores.mjs <package>.

import assert from "node:assert/strict";

import { fakeCoreClass, setup } from "./lib.mjs";

const { rt, types, errors, stores, KeelIds } = await setup(process.argv[2]);
const { KeelWriter, encodeValue, encodePatch, codecs, ChangeOp } = rt;
const FakeCore = fakeCoreClass(rt);

const core = new FakeCore();
core.nextHandle = 21n;
const todos = await stores.Todos.create(core);
const deliver = (id, codec, value) => core.deliver(21n, id, ChangeOp.FullValue, encodeValue(codec, value));

const todo = { id: "00000000-0000-0000-0000-00000000000a", title: "t", done: false };
deliver(0, codecs.vec(types.TodoCodec), [todo]);
deliver(1, types.FilterCodec, "active");
deliver(2, codecs.vec(types.TodoCodec), [todo, todo]);
deliver(3, codecs.u32, 5);
deliver(4, codecs.option(types.TodoCodec), todo);
deliver(5, codecs.string, "title");
deliver(6, types.CounterCodec, { label: "l", n: -1 });
deliver(7, codecs.map(codecs.string, codecs.u32), new Map([["x", 1]]));
deliver(8, codecs.option(errors.TodoErrorCodec), new errors.TodoError.Storage());
deliver(9, codecs.duration, 1500);
deliver(10, codecs.timestamp, 1700000000000);
deliver(11, codecs.bytes, new Uint8Array([7, 8]));
deliver(12, codecs.u64, 18446744073709551615n);
deliver(13, codecs.bool, true);
deliver(14, codecs.uuid, "00000000-0000-0000-0000-0000000000ff");

assert.deepEqual(todos.todos.get(), [todo]);
assert.equal(todos.filter.get(), "active");
assert.deepEqual(todos.visible.get(), [todo, todo]);
assert.equal(todos.remaining.get(), 5);
assert.deepEqual(todos.selected.get(), todo);
assert.equal(todos.title.get(), "title");
assert.deepEqual(todos.counter.get(), { label: "l", n: -1 });
assert.deepEqual(todos.tags.get(), new Map([["x", 1]]));
assert.ok(todos.lastError.get() instanceof errors.TodoError.Storage);
assert.equal(todos.elapsed.get(), 1500);
assert.equal(todos.created.get(), 1700000000000);
assert.deepEqual([...todos.blob.get()], [7, 8]);
assert.equal(todos.total.get(), 18446744073709551615n);
assert.equal(todos.default.get(), true);
assert.equal(todos.uuid.get(), "00000000-0000-0000-0000-0000000000ff");

// A computed keyed list is patched like a plain one.
const patch = new KeelWriter();
encodePatch(patch, [{ op: "clear" }, { op: "insert", index: 0, item: todo }], types.TodoCodec);
core.deliver(21n, 2, ChangeOp.KeyedPatch, patch.finish());
assert.deepEqual(todos.visible.get(), [todo]);

// Every signal is reachable through the base class list, in signal-id order.
assert.equal(todos._signals.length, 15);
assert.equal(todos._signals[3], todos.remaining);

// A store constructed with an argument.
const clock = await stores.Clock.create("UTC", core);
assert.deepEqual(core.constructed.at(-1), {
  typeId: KeelIds.Objects.Clock.typeId,
  methodId: KeelIds.Objects.Clock.new,
  args: "03000000" + "555443",
});
core.deliver(clock.handle, 0, ChangeOp.FullValue, encodeValue(codecs.timestamp, 42));
assert.equal(clock.now.get(), 42);

console.log("ok");
