// Execution test of the `lazy` golden case (ADR-043): a `Lazy<T>` signal is the runtime's lazy list, which the store
// hands its change-set entries. Usage: node lazy.mjs <package>.

import assert from "node:assert/strict";

import { fakeCoreClass, setup } from "./lib.mjs";

const { rt, stores } = await setup(process.argv[2]);
const { UndraWriter, ChangeOp } = rt;
const FakeCore = fakeCoreClass(rt);

const core = new FakeCore();
core.nextHandle = 4n;
const library = await stores.Library.create(core);

// `books` and `recent` are lists, not signals; the ordinary signals are still there.
assert.equal(typeof library.books.get, "function");
assert.equal(library.books.length.get(), 0);
assert.equal(library.recent.length.get(), 0);
assert.equal(library.total.get(), 0n);
assert.equal(library._signals.length, 3);

// `LazyValue`: handle u64, len u32, version u64; `LazyInvalidated`: len u32, version u64.
const value = (handle, len, version) => {
  const w = new UndraWriter();
  w.writeU64(handle);
  w.writeU32(len);
  w.writeU64(version);
  return w.finish();
};
const invalidated = (len, version) => {
  const w = new UndraWriter();
  w.writeU32(len);
  w.writeU64(version);
  return w.finish();
};
core.deliver(4n, 1, ChangeOp.FullValue, value(77n, 120, 1n));
assert.equal(library.books.length.get(), 120);
assert.equal(library.recent.length.get(), 0);
core.deliver(4n, 3, ChangeOp.FullValue, value(78n, 5, 1n));
assert.equal(library.recent.length.get(), 5);
core.deliver(4n, 1, ChangeOp.LazyInvalidated, invalidated(121, 2n));
assert.equal(library.books.length.get(), 121);
// A keyed patch is not something a lazy list takes: it is left alone.
core.deliver(4n, 1, ChangeOp.KeyedPatch, new Uint8Array(0));
assert.equal(library.books.length.get(), 121);
assert.deepEqual(core.reports, []);

// A malformed entry is reported and skipped, never half applied.
core.deliver(4n, 1, ChangeOp.FullValue, new Uint8Array([1, 2, 3]));
assert.equal(library.books.length.get(), 121);
assert.equal(core.reports.length, 1);
assert.match(core.reports[0].operation, /Library\.apply\(signal: 1\)/);

console.log("ok");
