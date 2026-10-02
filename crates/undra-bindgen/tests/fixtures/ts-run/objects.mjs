// Execution test of the `objects` golden case. Usage: node objects.mjs <package>.

import assert from "node:assert/strict";

import { bytes, fakeCoreClass, setup } from "./lib.mjs";

const { rt, types, errors, objects, UndraIds, core: core_ } = await setup(process.argv[2]);
const { encodeValue, codecs, CallTarget, ReplyStatus } = rt;
const FakeCore = fakeCoreClass(rt);
const replyError = (codec, value) => new rt.UndraReplyError(ReplyStatus.Error, encodeValue(codec, value));

const core = new FakeCore();

// Constructors: the plain one, a fallible one with an argument, an async fallible one.
const plain = await objects.Calculator.create(core);
assert.equal(core.constructed[0].args, "");
const precise = await objects.Calculator.withPrecision(3, core);
assert.deepEqual(core.constructed[1], {
  typeId: UndraIds.Objects.Calculator.typeId,
  methodId: UndraIds.Objects.Calculator.withPrecision,
  args: "03",
});
assert.equal(precise.handle, 8n);
core.replies.push(replyError(errors.CalcErrorCodec, new errors.CalcError.Overflow()));
await assert.rejects(objects.Calculator.withPrecision(200, core), (e) => e instanceof errors.CalcError.Overflow);
core.replies.push(replyError(errors.CalcErrorCodec, new errors.CalcError.DivideByZero()));
await assert.rejects(objects.Calculator.open("p", "exact", core), (e) => e instanceof errors.CalcError.DivideByZero);
assert.equal(core.constructed.at(-1).args, "01000000" + "70" + "0100");

// Parameters called like the generated locals do not clash with them.
core.replies.push(bytes("2a000000"));
assert.equal(await plain.delete(1, 2, 3, 4, 5), 42);
assert.deepEqual(core.calls.at(-1), {
  target: { target: CallTarget.ObjectMethod, handle: 7n },
  methodId: UndraIds.Objects.Calculator.delete,
  args: "0100000002000000030000000400000005000000",
  signal: undefined,
});

// Optional arguments, unit results and typed errors of synchronous methods.
core.replies.push(encodeValue(codecs.f64, 2.5));
assert.equal(await plain.compute(null), 2.5);
assert.equal(core.calls.at(-1).args, "00");
core.replies.push(encodeValue(codecs.f64, 3.5));
assert.equal(await plain.compute(1.5), 3.5);
assert.equal(core.calls.at(-1).args, "01" + "000000000000f83f");
await plain.warmUp();
await plain.reset();
await plain.check();
core.replies.push(replyError(errors.CalcErrorCodec, new errors.CalcError.DivideByZero()));
await assert.rejects(plain.divide(1n, 0n), (e) => e instanceof errors.CalcError.DivideByZero);
assert.equal(core.calls.at(-1).args, "0100000000000000" + "0000000000000000");
core.replies.push(encodeValue(codecs.i64, -6n));
assert.equal(await plain.divide(12n, -2n), -6n);
core.replies.push(replyError(errors.CalcErrorCodec, new errors.CalcError.Overflow()));
await assert.rejects(plain.check(), (e) => e instanceof errors.CalcError.Overflow);

core.replies.push(encodeValue(types.StatsCodec, { count: 3, total: 9n }));
assert.deepEqual(await plain.stats(), { count: 3, total: 9n });

// Streams of free functions and of methods.
core.streams.push([bytes("04000000")]);
const upto = [];
for await (const n of objects.numbers(4, core)) upto.push(n);
assert.deepEqual(upto, [4]);
assert.deepEqual(core.calls.at(-1).target, { target: CallTarget.FreeFunction });
assert.equal(core.calls.at(-1).methodId, UndraIds.Functions.numbers);
core.streams.push([]);
const none = [];
for await (const item of plain.watch("fast")) none.push(item);
assert.deepEqual(none, []);

// Releasing a handle goes through the base class.
plain.close();

// The generated entry (ADR-044): while its core is not loaded it is the closed placeholder; `attach` passes
// this package's schema hash and makes the attached core the default of every generated API; a second
// attach while it is open is refused; once it is closed the placeholder is back.
const { UndraGoldenObjects } = core_;
const placeholder = new FakeCore();
Object.defineProperty(rt.UndraCore, "unloaded", { get: () => placeholder });
assert.equal(UndraGoldenObjects.core, placeholder);
assert.equal(UndraGoldenObjects.namespace, "golden_objects");
assert.equal(UndraIds.namespace, "golden_objects");
const attached = new FakeCore();
let seen;
rt.UndraCore.attach = async (transport, options) => {
  seen = { transport, options };
  return attached;
};
const transport = { mode: "test" };
assert.equal(await UndraGoldenObjects.attach(transport, { shared: false }), attached);
// The entry fills in the schema hash and the core's namespace (ADR-044 amendment A: the default stores are per namespace).
assert.deepEqual(seen, { transport, options: { shared: false, expectedSchemaHash: UndraIds.schemaHash, namespace: "golden_objects" } });
assert.equal(UndraGoldenObjects.core, attached);
await assert.rejects(UndraGoldenObjects.attach(transport), (e) => e instanceof rt.UndraError && e.kind === "state");
await objects.Calculator.create();
assert.equal(attached.constructed.length, 1, "a generated constructor without a core uses the entry's");
Object.defineProperty(attached, "closed", { get: () => true });
assert.equal(UndraGoldenObjects.core, placeholder);

console.log("ok");
