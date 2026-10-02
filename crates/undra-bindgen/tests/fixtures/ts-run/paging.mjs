// Execution test of the `infinite` golden case (ADR-043): an infinite query's handle pages and polls through
// commands, and its rows arrive as keyed patches. Usage: node paging.mjs <package>.

import assert from "node:assert/strict";

import { fakeCoreClass, hex, setup } from "./lib.mjs";

const { rt, types, queries, UndraIds } = await setup(process.argv[2]);
const { UndraWriter, ChangeOp, codecs, encodePatch, encodeValue } = rt;
const FakeCore = fakeCoreClass(rt);

const core = new FakeCore();
core.nextHandle = 3n;
const feed = await queries.FeedQueryHandle.create("all", core);
const ids = UndraIds.Objects.FeedQueryHandle;
assert.deepEqual(core.constructed.at(-1), { typeId: ids.typeId, methodId: ids.new, args: "0000" });

// The placeholders: no rows, no next page.
assert.deepEqual(feed.data.get(), []);
assert.equal(feed.hasNextPage.get(), false);
assert.equal(feed.fetchingNextPage.get(), false);

// The commands: each is a call of its fixed method id.
await feed.fetchNextPage();
assert.deepEqual(core.calls.at(-1).methodId, ids.fetchNextPage);
assert.equal(core.calls.at(-1).args, "");
await feed.setPollInterval(1500);
assert.equal(core.calls.at(-1).methodId, ids.setPollInterval);
assert.equal(core.calls.at(-1).args, "01" + "002f685900000000");
await feed.setPollInterval(null);
assert.equal(core.calls.at(-1).args, "00");
await feed.refetch();
assert.equal(core.calls.at(-1).methodId, ids.refetch);
assert.deepEqual(core.reports, []);

// The rows: a full value, then a next page arriving as a patch that appends.
const post = (id) => ({ id: types.PostId(BigInt(id)), author: "a", body: `b${id}` });
core.deliver(3n, 0, ChangeOp.FullValue, encodeValue(codecs.vec(types.PostCodec), [post(1), post(2)]));
assert.deepEqual(feed.data.get(), [post(1), post(2)]);
const page = new UndraWriter();
encodePatch(page, [{ op: "insert", index: 2, item: post(3) }, { op: "insert", index: 3, item: post(4) }], types.PostCodec);
core.deliver(3n, 0, ChangeOp.KeyedPatch, page.finish());
assert.deepEqual(feed.data.get().map((p) => p.id), [1n, 2n, 3n, 4n]);
core.deliver(3n, 5, ChangeOp.FullValue, encodeValue(codecs.bool, true));
core.deliver(3n, 6, ChangeOp.FullValue, encodeValue(codecs.bool, true));
assert.equal(feed.hasNextPage.get(), true);
assert.equal(feed.fetchingNextPage.get(), true);

// An ordinary handle has neither.
const profile = await queries.ProfileQueryHandle.create(core);
assert.equal("fetchNextPage" in profile, false);
assert.equal("hasNextPage" in profile, false);
await profile.setPollInterval(2000);
assert.equal(core.calls.at(-1).methodId, UndraIds.Objects.ProfileQueryHandle.setPollInterval);

console.log("ok");
