import assert from "node:assert/strict";

import { bytes, fakeCoreClass, hex, setup } from "./lib.mjs";

const { rt, types, errors, objects, stores, ports, queries, UndraIds } = await setup(process.argv[2]);
const { UndraWriter, encodeValue, decodeValue, encodePatch, codecs, CallTarget, ChangeOp, ReplyStatus } = rt;
const FakeCore = fakeCoreClass(rt);

const todo = {
  id: "00112233-4455-6677-8899-aabbccddeeff",
  title: "Milk",
  done: true,
  tags: ["a", "bc"],
  due: 1700000000000,
  priority: "high",
};
const todoBytes =
  "00112233445566778899aabbccddeeff" + // id
  "04000000" + "4d696c6b" + // title
  "01" + // done
  "02000000" + "0100000061" + "020000006263" + // tags
  "01" + "0068e5cf8b010000" + // due: Some(1700000000000)
  "0200"; // priority: High

// ----- records and enums ---------------------------------------------------------------

assert.equal(hex(encodeValue(types.TodoCodec, todo)), todoBytes);
assert.deepEqual(decodeValue(types.TodoCodec, bytes(todoBytes)), todo);
assert.deepEqual(decodeValue(types.TodoCodec, bytes(todoBytes.replace("01" + "0068e5cf8b010000", "00")))
  .due, null);

const circle = { kind: "circle", radius: 1.5 };
assert.equal(hex(encodeValue(types.ShapeCodec, circle)), "0000" + "000000000000f83f");
assert.deepEqual(decodeValue(types.ShapeCodec, bytes("0100" + "000000000000f83f" + "000000000000f83f")), {
  kind: "rect",
  w: 1.5,
  h: 1.5,
});
assert.deepEqual(decodeValue(types.ShapeCodec, bytes("0200")), { kind: "empty" });
assert.throws(() => decodeValue(types.ShapeCodec, bytes("0900")), (e) => e.code === "invalid_tag" && e.detail.ty === "Shape");
assert.equal(hex(encodeValue(types.FilterCodec, "done")), "0200");
assert.throws(() => decodeValue(types.FilterCodec, bytes("0700")), (e) => e.code === "invalid_tag");
assert.throws(() => decodeValue(types.TodoCodec, bytes(todoBytes + "00")), (e) => e.code === "trailing_bytes");

const page = { items: [todo, todo], next: null, total: 18446744073709551615n };
assert.deepEqual(decodeValue(types.PageCodec, encodeValue(types.PageCodec, page)), page);
const request = { method: "GET", url: "/", headers: new Map([["b", "2"], ["a", "1"]]), body: new Uint8Array([1, 2, 3]) };
const requestBytes = encodeValue(types.HttpRequestCodec, request);
assert.deepEqual(decodeValue(types.HttpRequestCodec, requestBytes), request);
// Map entries are written sorted by their encoded key, whatever the insertion order.
const sortedHeaders = new Map([["a", "1"], ["b", "2"]]);
assert.equal(
  hex(encodeValue(types.HttpRequestCodec, { ...request, headers: sortedHeaders })),
  hex(requestBytes),
);

// ----- errors ----------------------------------------------------------------------------

const notFound = new errors.TodoError.NotFound("x1");
assert.equal(notFound.kind, "notFound");
assert.equal(notFound.message, "todo x1 not found");
assert.equal(notFound.value, "x1");
assert.ok(notFound instanceof errors.TodoError && notFound instanceof rt.UndraError && notFound instanceof Error);
assert.ok(!(notFound instanceof errors.TodoError.EmptyTitle));

const wrapped = new errors.TodoError.Http(new errors.HttpError.Status(404));
assert.equal(wrapped.kind, "http");
assert.equal(wrapped.message, "status 404");
assert.equal(wrapped.cause.kind, "status");
assert.equal(hex(encodeValue(errors.TodoErrorCodec, wrapped)), "0100" + "0100" + "9401");
const decodedWrapped = decodeValue(errors.TodoErrorCodec, bytes("0100" + "0100" + "9401"));
assert.ok(decodedWrapped instanceof errors.TodoError.Http);
assert.equal(decodedWrapped.cause.value, 404);
assert.equal(hex(encodeValue(errors.TodoErrorCodec, new errors.TodoError.Storage("disk"))), "0300" + "04000000" + "6469736b");

const replyError = (codec, value) => new rt.UndraReplyError(ReplyStatus.Error, encodeValue(codec, value));
const strings = (...text) => {
  const w = new UndraWriter();
  for (const t of text) w.writeStr(t);
  return w.finish();
};
const panicBody = (message, backtrace) => strings(message, backtrace);
const typed = rt.UndraCallError.mapped(replyError(errors.TodoErrorCodec, new errors.TodoError.EmptyTitle()), errors.TodoErrorCodec);
assert.ok(typed instanceof errors.TodoError.EmptyTitle);
const panic = new rt.UndraReplyError(ReplyStatus.Panic, panicBody("boom", "frame"));
assert.ok(rt.UndraCallError.mapped(panic, errors.TodoErrorCodec) instanceof rt.UndraCallError.Panicked, "other replies map onto the closed set");
const plain = new RangeError("plain");
assert.equal(rt.UndraCallError.mapped(plain, errors.TodoErrorCodec), plain, "a failure that is not Undra's stays itself");

// ----- objects ----------------------------------------------------------------------------

{
  const core = new FakeCore();
  const calc = await objects.Calculator.create(core);
  assert.deepEqual(core.constructed, [
    { typeId: UndraIds.Objects.Calculator.typeId, methodId: UndraIds.Objects.Calculator.new, args: "" },
  ]);
  assert.equal(calc.handle, 7n);
  assert.equal(core.observed.length, 0, "a plain object is not observed");

  core.replies.push(bytes("05000000"));
  assert.equal(await calc.add(2, 3), 5);
  assert.deepEqual(core.calls[0], {
    target: { target: CallTarget.ObjectMethod, handle: 7n },
    methodId: UndraIds.Objects.Calculator.add,
    args: "0200000003000000",
    signal: undefined,
  });

  // A `Result<String, HttpError>` method rejects with the typed error.
  core.replies.push(replyError(errors.HttpErrorCodec, new errors.HttpError.Timeout()));
  await assert.rejects(calc.fetch("https://example.com"), (e) => e instanceof errors.HttpError.Timeout);
  assert.equal(core.calls[1].args, "13000000" + Buffer.from("https://example.com").toString("hex"));
  core.replies.push(encodeValue(codecs.string, "ok"));
  assert.equal(await calc.fetch("u"), "ok");
  // Anything but an error reply maps onto the closed set (ADR-032, amendment A).
  core.replies.push(new rt.UndraReplyError(ReplyStatus.Panic, panicBody("kaboom", "frame")));
  await assert.rejects(calc.fetch("u"), (e) => e instanceof rt.UndraCallError.Panicked && e.panicMessage === "kaboom" && e.backtrace === "frame");
  core.replies.push(new rt.UndraReplyError(ReplyStatus.Cancelled, new Uint8Array(0)));
  await assert.rejects(calc.fetch("u"), (e) => e instanceof rt.UndraCallError.CancelledByCore);
  core.replies.push(new rt.UndraReplyError(ReplyStatus.BadRequest, strings("stale handle")));
  await assert.rejects(calc.fetch("u"), (e) => e instanceof rt.UndraCallError.Refused && e.reason === "stale handle");
  core.replies.push(new rt.UndraTransportError("closed", "the core is closed"));
  await assert.rejects(calc.fetch("u"), (e) => e instanceof rt.UndraCallError.Unavailable && e.transport.reason === "closed");
  core.replies.push(encodeValue(codecs.u32, 1));
  await assert.rejects(calc.fetch("u"), (e) => e instanceof rt.UndraCallError.Malformed, "a result that does not decode");
  // The caller's own cancellation is not an UndraCallError.
  const abort = new DOMException("The operation was aborted", "AbortError");
  core.replies.push(abort);
  await assert.rejects(calc.fetch("u"), (e) => e === abort);
  // A call without an error type maps the same way.
  core.replies.push(new rt.UndraReplyError(ReplyStatus.Panic, panicBody("sync boom", "")));
  await assert.rejects(calc.add(1, 2), (e) => e instanceof rt.UndraCallError.Panicked && e.panicMessage === "sync boom");

  const controller = new AbortController();
  core.replies.push(encodeValue(codecs.string, "ok"));
  await calc.fetch("u", controller.signal);
  assert.equal(core.calls.at(-1).signal, controller.signal);

  // Streams decode each item.
  core.streams.push([bytes("01000000"), bytes("02000000"), bytes("03000000")]);
  const ticks = [];
  for await (const tick of calc.ticks()) ticks.push(tick);
  assert.deepEqual(ticks, [1, 2, 3]);
  // A typed failure of a `Result<Stream<_>, E>` surfaces while iterating.
  core.streams.push([encodeValue(types.TodoCodec, todo), replyError(errors.TodoErrorCodec, new errors.TodoError.EmptyTitle())]);
  const seen = [];
  await assert.rejects(
    (async () => {
      for await (const item of calc.watch("high")) seen.push(item);
    })(),
    (e) => e instanceof errors.TodoError.EmptyTitle,
  );
  assert.deepEqual(seen, [todo]);
  assert.equal(core.calls.at(-1).args, "0200");

  // The core ending a stream itself (ADR-036, SPEC 3.7): a flag-3 failure arrives as the failed reply with its status.
  const failing = async (iterate) => {
    try {
      for await (const _ of iterate()) void _;
    } catch (error) {
      return error;
    }
    return undefined;
  };
  core.streams.push([new rt.UndraReplyError(ReplyStatus.Cancelled, new Uint8Array(0))]);
  assert.ok((await failing(() => calc.watch("high"))) instanceof rt.UndraCallError.CancelledByCore, "a stream ended by a restore");
  core.streams.push([new rt.UndraReplyError(ReplyStatus.Cancelled, new Uint8Array(0))]);
  assert.ok((await failing(() => calc.ticks())) instanceof rt.UndraCallError.CancelledByCore, "a stream without an error type ended by shutdown");
  core.streams.push([new rt.UndraReplyError(ReplyStatus.Panic, panicBody("the stream panicked: boom", "at core.rs:1"))]);
  const panicked = await failing(() => calc.ticks());
  assert.ok(panicked instanceof rt.UndraCallError.Panicked && panicked.panicMessage === "the stream panicked: boom");
  assert.equal(panicked.backtrace, "at core.rs:1");
  core.streams.push([new rt.UndraReplyError(ReplyStatus.BadRequest, strings("stale handle"))]);
  const refused = await failing(() => calc.ticks());
  assert.ok(refused instanceof rt.UndraCallError.Refused && refused.reason === "stale handle", "a refused stream");
  core.streams.push([new rt.UndraReplyError(ReplyStatus.BadRequest, strings("stale handle"))]);
  assert.ok((await failing(() => calc.watch("high"))) instanceof rt.UndraCallError.Refused, "a refused stream with an error type");
  // A typed error item on a stream that has no error type is not the core's text: Malformed.
  core.streams.push([new rt.UndraReplyError(ReplyStatus.Error, strings("the stream panicked: boom"))]);
  assert.ok((await failing(() => calc.ticks())) instanceof rt.UndraCallError.Malformed, "a typed error item on a stream without an error type");
  core.streams.push([new rt.UndraTransportError("closed", "closed")]);
  assert.ok((await failing(() => calc.ticks())) instanceof rt.UndraCallError.Unavailable, "a stream on a closed core");
  core.streams.push([bytes("01")]);
  assert.ok((await failing(() => calc.ticks())) instanceof rt.UndraCallError.Malformed, "a stream item that does not decode");

  core.replies.push(encodeValue(codecs.string, "hello, undra"));
  assert.equal(await objects.greet("undra", core), "hello, undra");
  assert.deepEqual(core.calls.at(-1).target, { target: CallTarget.FreeFunction });
  assert.equal(core.calls.at(-1).methodId, UndraIds.Functions.greet);
  core.replies.push(replyError(errors.TodoErrorCodec, new errors.TodoError.EmptyTitle()));
  await assert.rejects(objects.ping(core), (e) => e instanceof errors.TodoError.EmptyTitle);
  core.replies.push(new rt.UndraReplyError(ReplyStatus.Panic, panicBody("later", "")));
  await assert.rejects(objects.ping(core), (e) => e instanceof rt.UndraCallError.Panicked, "a failure that is not the function's own error");
  core.replies.push(new rt.UndraReplyError(ReplyStatus.Panic, panicBody("sync fn", "")));
  await assert.rejects(objects.greet("undra", core), (e) => e instanceof rt.UndraCallError.Panicked);

  // Constructors are calls: a refused one is an UndraCallError, a typed one is the error.
  core.replies.push(new rt.UndraReplyError(ReplyStatus.BadRequest, strings("undecodable arguments")));
  await assert.rejects(objects.Calculator.create(core), (e) => e instanceof rt.UndraCallError.Refused);
}

// ----- stores -----------------------------------------------------------------------------

{
  const core = new FakeCore();
  core.nextHandle = 9n;
  const store = await stores.TodoStore.create(core);
  assert.deepEqual(core.observed, [{ handle: 9n, signalId: 0xffff_ffff, on: true }]);
  // Placeholders until the first change-set.
  assert.deepEqual(store.todos.get(), []);
  assert.equal(store.filter.get(), "all");
  assert.equal(store.remaining.get(), 0);
  assert.equal(store.selected.get(), null);

  const todoList = codecs.vec(types.TodoCodec);
  core.deliver(9n, 0, ChangeOp.FullValue, encodeValue(todoList, [todo]));
  core.deliver(9n, 1, ChangeOp.FullValue, encodeValue(types.FilterCodec, "done"));
  core.deliver(9n, 3, ChangeOp.FullValue, encodeValue(codecs.u32, 4));
  core.deliver(9n, 4, ChangeOp.FullValue, encodeValue(codecs.option(types.TodoCodec), todo));
  assert.deepEqual(store.todos.get(), [todo]);
  assert.equal(store.filter.get(), "done");
  assert.equal(store.remaining.get(), 4);
  assert.deepEqual(store.selected.get(), todo);

  // Keyed patches apply to the current list; unchanged items keep their identity.
  const first = store.todos.get()[0];
  const second = { ...todo, id: "ffffffff-4455-6677-8899-aabbccddeeff", title: "Bread" };
  const patch = new UndraWriter();
  encodePatch(patch, [{ op: "insert", index: 1, item: second }], types.TodoCodec);
  core.deliver(9n, 0, ChangeOp.KeyedPatch, patch.finish());
  assert.equal(store.todos.get().length, 2);
  assert.equal(store.todos.get()[0], first);
  assert.equal(store.todos.get()[1].title, "Bread");

  const move = new UndraWriter();
  encodePatch(move, [{ op: "move", from: 1, to: 0 }, { op: "remove", index: 1 }], types.TodoCodec);
  core.deliver(9n, 0, ChangeOp.KeyedPatch, move.finish());
  assert.deepEqual(store.todos.get().map((t) => t.title), ["Bread"]);

  // A patch that does not fit desynchronises the signal: it is re-observed for a full value.
  core.observed.length = 0;
  const bad = new UndraWriter();
  encodePatch(bad, [{ op: "remove", index: 9 }], types.TodoCodec);
  core.deliver(9n, 0, ChangeOp.KeyedPatch, bad.finish());
  assert.deepEqual(core.observed, [
    { handle: 9n, signalId: 0, on: false },
    { handle: 9n, signalId: 0, on: true },
  ]);
  assert.deepEqual(store.todos.get().map((t) => t.title), ["Bread"]);

  // Unknown signals and lazy invalidations are ignored.
  core.deliver(9n, 42, ChangeOp.FullValue, new Uint8Array(0));
  core.deliver(9n, 1, ChangeOp.LazyInvalidated, new Uint8Array(0));
  assert.equal(store.filter.get(), "done");

  const seenFilters = [];
  store.filter.subscribe((f) => seenFilters.push(f));
  core.deliver(9n, 1, ChangeOp.FullValue, encodeValue(types.FilterCodec, "active"));
  assert.deepEqual(seenFilters, ["active"]);

  await store.setFilter("all");
  assert.deepEqual(core.calls.at(-1), {
    target: { target: CallTarget.ObjectMethod, handle: 9n },
    methodId: UndraIds.Objects.TodoStore.setFilter,
    args: "0000",
    signal: undefined,
  });
  assert.equal(core.reports.length, 0, "a command that succeeds reports nothing");

  // A command never rejects: a failure is reported with the operation as TypeScript spells it, and the promise resolves.
  core.replies.push(new rt.UndraReplyError(ReplyStatus.BadRequest, strings("stale handle")));
  await store.setFilter("done");
  core.replies.push(new rt.UndraTransportError("closed", "the core is closed"));
  await store.toggle("00112233-4455-6677-8899-aabbccddeeff");
  assert.deepEqual(core.reports.map((r) => r.operation), ["TodoStore.setFilter", "TodoStore.toggle"]);
  assert.ok(core.reports[0].error instanceof rt.UndraReplyError, "the raw failure is handed to report, which maps it");
  core.reports.length = 0;

  // A change that does not decode is reported and skipped, never half applied.
  core.deliver(9n, 3, ChangeOp.FullValue, new Uint8Array([1]));
  assert.equal(store.remaining.get(), 4, "an undecodable value is skipped");
  core.deliver(9n, 3, ChangeOp.FullValue, new Uint8Array([...encodeValue(codecs.u32, 7), 0]));
  assert.equal(store.remaining.get(), 4, "a value with trailing bytes is skipped whole");
  assert.deepEqual(core.reports.map((r) => r.operation), ["TodoStore.apply(signal: 3)", "TodoStore.apply(signal: 3)"]);
  core.reports.length = 0;

  // A store whose observe fails is closed and the failure is an UndraCallError.
  const gone = new FakeCore();
  gone.observe = async () => {
    throw new rt.UndraTransportError("closed", "the core is closed");
  };
  await assert.rejects(stores.TodoStore.create(gone), (e) => e instanceof rt.UndraCallError.Unavailable);

  // A fallible async constructor maps a typed failure.
  core.replies.push(replyError(errors.TodoErrorCodec, new errors.TodoError.NotFound("db")));
  await assert.rejects(stores.TodoStore.open("/tmp/db", core), (e) => e instanceof errors.TodoError.NotFound);
}

// ----- ports ------------------------------------------------------------------------------

{
  const clock = ports.wallClockPortImpl({ nowMs: () => 5n, monotonicNs: () => 6n });
  assert.equal(clock.sync, true);
  assert.equal(hex(clock.methods[UndraIds.Ports.WallClock.nowMs]()), "0500000000000000");
  assert.equal(hex(clock.methods[UndraIds.Ports.WallClock.monotonicNs]()), "0600000000000000");

  let seenRequest;
  const http = ports.httpPortImpl({
    request: async (req) => {
      seenRequest = req;
      if (req.url === "/timeout") throw new errors.HttpError.Timeout();
      if (req.url === "/crash") throw new RangeError("boom");
      return { status: 200, headers: new Map(), body: new Uint8Array([9]), elapsed: 2 };
    },
  });
  assert.equal(http.sync, false);
  const method = http.methods[UndraIds.Ports.Http.request];
  const reply = await method(encodeValue(types.HttpRequestCodec, { ...request, url: "/ok" }));
  assert.equal(seenRequest.url, "/ok");
  assert.deepEqual(decodeValue(types.HttpResponseCodec, reply), {
    status: 200,
    headers: new Map(),
    body: new Uint8Array([9]),
    elapsed: 2,
  });
  await assert.rejects(
    method(encodeValue(types.HttpRequestCodec, { ...request, url: "/timeout" })),
    (e) => e instanceof rt.UndraPortError && hex(e.body) === "0000",
  );
  await assert.rejects(
    method(encodeValue(types.HttpRequestCodec, { ...request, url: "/crash" })),
    (e) => e instanceof RangeError,
  );
  // Malformed arguments are a wire error, not a crash.
  await assert.rejects(method(new Uint8Array([1])), (e) => e.name === "WireError");

  const stored = new Map();
  const kv = ports.kvPortImpl({
    get: async (key) => stored.get(key) ?? null,
    set: async (key, value) => void stored.set(key, value),
  });
  const set = kv.methods[UndraIds.Ports.Kv.set];
  const w = new UndraWriter();
  w.writeStr("k");
  w.writeBytes(new Uint8Array([1, 2]));
  assert.equal(hex(await set(w.finish())), "");
  assert.deepEqual([...stored.get("k")], [1, 2]);
  const get = kv.methods[UndraIds.Ports.Kv.get];
  const key = new UndraWriter();
  key.writeStr("k");
  assert.equal(hex(await get(key.finish())), "01" + "02000000" + "0102");
  const missing = new UndraWriter();
  missing.writeStr("nope");
  assert.equal(hex(await get(missing.finish())), "00");

  const core = new FakeCore();
  new ports.ConnectivityEvents(core).changed(true, "wifi");
  assert.deepEqual(core.events, [
    { portId: UndraIds.Ports.Connectivity.portId, methodId: UndraIds.Ports.Connectivity.changed, payload: "010000" },
  ]);
}

// ----- queries ----------------------------------------------------------------------------

{
  const core = new FakeCore();
  core.nextHandle = 3n;
  const handle = await queries.TodosQueryHandle.create(5, core);
  assert.deepEqual(core.constructed, [
    { typeId: UndraIds.Queries.todos, methodId: UndraIds.Queries.todos, args: "05000000" },
  ]);
  assert.deepEqual(core.observed, [{ handle: 3n, signalId: 0xffff_ffff, on: true }]);
  assert.equal(handle.status.get(), "idle");
  assert.equal(handle.data.get(), null);
  core.deliver(3n, 1, ChangeOp.FullValue, encodeValue(types.QueryStatusCodec, "success"));
  core.deliver(3n, 3, ChangeOp.FullValue, encodeValue(codecs.bool, true));
  core.deliver(3n, 2, ChangeOp.FullValue, encodeValue(codecs.option(errors.TodoErrorCodec), new errors.TodoError.EmptyTitle()));
  assert.equal(handle.status.get(), "success");
  assert.equal(handle.fetching.get(), true);
  assert.ok(handle.error.get() instanceof errors.TodoError.EmptyTitle);
  assert.equal(handle.updatedAt.get(), null);

  await handle.refetch();
  assert.equal(core.calls.at(-1).methodId, 0x21d1b9e2);
  await handle.invalidate();
  assert.equal(core.calls.at(-1).methodId, 0x44cec2fa);

  core.replies.push(encodeValue(types.TodoCodec, todo));
  assert.deepEqual(await queries.addTodo("Milk", core), todo);
  assert.deepEqual(core.calls.at(-1).target, { target: CallTarget.FreeFunction });
  assert.equal(core.calls.at(-1).methodId, UndraIds.Queries.addTodo);
}

// ----- ids --------------------------------------------------------------------------------

assert.equal(UndraIds.Objects.Calculator.add, 2353348832);
assert.equal(typeof UndraIds.schemaHash, "bigint");

console.log("ok");
