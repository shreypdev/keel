// Execution test of the `object_graph` golden case (ADR-040). Usage: node object_graph.mjs <package>.

import assert from "node:assert/strict";

import { fakeCoreClass, hex, setup } from "./lib.mjs";

const { rt, objects, stores, UndraIds } = await setup(process.argv[2]);
const { encodeValue, codecs, CallTarget } = rt;
const FakeCore = fakeCoreClass(rt);
const handle = (h) => encodeValue(codecs.u64, h);

const core = new FakeCore();

// A constructor goes through `adopt`.
const account = await objects.Account.create(core);
assert.equal(account.handle, 7n);

// A method returns an object: one wrapper per handle; the duplicate reference is given back at once.
core.replies.push(handle(40n));
const inbox = await account.mailbox("inbox");
assert.ok(inbox instanceof objects.Mailbox);
assert.equal(inbox.handle, 40n);
assert.equal(inbox.core, core);
core.replies.push(handle(40n));
assert.equal(await account.mailbox("inbox"), inbox, "the same handle is the same wrapper");
assert.deepEqual(core.givenBack, [40n], "the second reference went back");

// `Option` and `Vec` of objects.
core.replies.push(encodeValue(codecs.option(codecs.u64), null));
assert.equal(await account.drafts(), null);
core.replies.push(encodeValue(codecs.option(codecs.u64), 41n));
const drafts = await account.drafts();
assert.equal(drafts.handle, 41n);
core.replies.push(encodeValue(codecs.vec(codecs.u64), [40n, 41n, 42n]));
const boxes = await account.mailboxes();
assert.equal(boxes[0], inbox);
assert.equal(boxes[1], drafts);
assert.equal(boxes[2].handle, 42n);
assert.deepEqual(core.givenBack, [40n, 40n, 41n]);

// A store returned by a method is mirrored and observed once, like a constructed one.
core.replies.push(handle(50n));
const chat = await account.chat(3);
assert.ok(chat instanceof stores.ChatStore);
assert.ok(core.mirrorFns.has(50n));
assert.deepEqual(core.observed.at(-1), { handle: 50n, signalId: 0xffffffff, on: true });
const observations = core.observed.length;
core.replies.push(handle(50n));
assert.equal(await account.chat(3), chat);
assert.equal(core.observed.length, observations, "a store returned twice is observed once");

// An async method that fails with its typed error returns nothing to adopt.
core.replies.push(handle(60n));
const thread = await account.openThread(1);
assert.equal(thread.handle, 60n);

// Objects as parameters: the handle is written (borrowed).
await account.moveTo(9, inbox);
assert.deepEqual(core.calls.at(-1), {
  target: { target: CallTarget.ObjectMethod, handle: 7n },
  methodId: UndraIds.Objects.Account.moveTo,
  args: "09000000" + "2800000000000000",
  signal: undefined,
});
core.replies.push(encodeValue(codecs.u32, 2));
assert.equal(await account.merge([inbox, drafts], null), 2);
assert.equal(core.calls.at(-1).args, "02000000" + "2800000000000000" + "2900000000000000" + "00");
core.replies.push(encodeValue(codecs.u32, 1));
await account.merge([], boxes[2]);
assert.equal(core.calls.at(-1).args, "00000000" + "01" + "2a00000000000000");

// A free function takes and returns objects.
core.replies.push(handle(40n));
assert.equal(await objects.mailboxOf(account, "inbox", core), inbox);
assert.equal(core.calls.at(-1).args, "0700000000000000" + hex(encodeValue(codecs.string, "inbox")));

// An object of another core is refused before anything is sent: a call rejects, a command reports.
const other = new FakeCore();
const otherAccount = await objects.Account.create(other);
other.replies.push(handle(40n));
const foreign = await otherAccount.mailbox("inbox");
const calls = core.calls.length;
await assert.rejects(
  account.merge([foreign], null),
  (e) => e instanceof rt.UndraCallError.Refused && /Mailbox belongs to another core/.test(e.message),
);
await account.moveTo(1, foreign);
assert.equal(core.calls.length, calls, "nothing was sent");
assert.equal(core.reports.at(-1).operation, "Account.moveTo");
assert.ok(core.reports.at(-1).error instanceof rt.UndraCallError.Refused);

// A closed wrapper is replaced: the handle the core returns again belongs to a new wrapper.
inbox.close();
core.replies.push(handle(40n));
const reopened = await account.mailbox("inbox");
assert.notEqual(reopened, inbox);
assert.equal(reopened.handle, 40n);

// The null handle where an object is expected is malformed.
core.replies.push(handle(0n));
await assert.rejects(account.mailbox("x"), (e) => e instanceof rt.UndraCallError.Malformed);

console.log("ok");
