// Execution test of the `newtypes` golden case (ADR-042): a newtype is its inner value on the wire, in
// every container, as a map key and as the key of a keyed list. Usage: node newtypes.mjs <package>.

import assert from "node:assert/strict";

import { fakeCoreClass, hex, setup } from "./lib.mjs";

const { rt, types, stores } = await setup(process.argv[2]);
const { UndraWriter, ChangeOp, codecs, decodeValue, encodePatch, encodeValue } = rt;
const t = types;

const uuid = "00000000-0000-0000-0000-0000000000ab";
const same = (codec, value, inner, innerValue) =>
  assert.equal(hex(encodeValue(codec, value)), hex(encodeValue(inner, innerValue)));

// The bytes are the inner value's: no tag, no length.
same(t.UserIdCodec, t.UserId(uuid), codecs.uuid, uuid);
same(t.TodoIdCodec, t.TodoId("t"), codecs.string, "t");
same(t.OrderNoCodec, t.OrderNo(7n), codecs.u64, 7n);
same(t.MetersCodec, t.Meters(1.5), codecs.f64, 1.5);
same(t.TimeoutCodec, t.Timeout(1500), codecs.duration, 1500);
same(t.CreatedCodec, t.Created(1700000000000), codecs.timestamp, 1700000000000);
same(t.FlagCodec, t.Flag(true), codecs.bool, true);
same(t.BlobCodec, t.Blob(new Uint8Array([1, 2, 3])), codecs.bytes, new Uint8Array([1, 2, 3]));
same(t.TagsCodec, t.Tags(["a", "b"]), codecs.vec(codecs.string), ["a", "b"]);
same(t.NicknameCodec, t.Nickname("n"), codecs.option(codecs.string), "n");
assert.equal(hex(encodeValue(t.NicknameCodec, t.Nickname(null))), "00");
same(t.LevelCodec, t.Level("high"), t.PriorityCodec, "high");
// Newtypes of newtypes, and an option of one, are the same bytes again.
const boss = t.Boss(t.Owner(t.UserId(uuid)));
same(t.BossCodec, boss, codecs.uuid, uuid);
same(t.MaybeCodec, t.Maybe(t.UserId(uuid)), codecs.option(codecs.uuid), uuid);
assert.equal(hex(encodeValue(t.MaybeCodec, t.Maybe(null))), "00");
const todo = { id: t.TodoId("t"), title: "x", owner: t.UserId(uuid), due: t.Created(5), level: t.Level("low") };
same(t.WrappedCodec, t.Wrapped(todo), t.TodoCodec, todo);
const ratings = t.Ratings(new Map([[t.UserId(uuid), t.Meters(2)]]));
same(t.RatingsCodec, ratings, codecs.map(codecs.uuid, codecs.f64), new Map([[uuid, 2]]));

// The brand is only a type: what comes back is the plain value.
assert.equal(decodeValue(t.UserIdCodec, encodeValue(codecs.uuid, uuid)), uuid);
assert.equal(decodeValue(t.BossCodec, encodeValue(codecs.uuid, uuid)), uuid);
assert.equal(decodeValue(t.NicknameCodec, bytes("00")), null);

// A whole record round trips, map keys included.
const account = {
  id: t.UserId(uuid),
  nickname: t.Nickname(null),
  tags: t.Tags(["x"]),
  boss: boss,
  friends: [t.UserId(uuid)],
  scores: new Map([[t.UserId(uuid), t.Meters(3)]]),
  byOrder: new Map([[t.OrderNo(9n), t.UserId(uuid)]]),
  timeout: t.Timeout(1),
  blob: t.Blob(new Uint8Array([9])),
  flag: t.Flag(false),
  maybe: t.Maybe(null),
  reach: t.Span(t.Meters(4)),
};
const back = decodeValue(t.AccountCodec, encodeValue(t.AccountCodec, account));
assert.deepEqual(back, account);
assert.equal(back.scores.get(uuid), 3);
assert.equal(back.byOrder.get(9n), uuid);

// A truncated newtype fails as its inner value does.
assert.throws(() => decodeValue(t.UserIdCodec, new Uint8Array(3)), (e) => e.code === "unexpected_eof");

// The placeholders of a store are made with the constructor functions, and a keyed list whose key
// is a newtype applies its patches like any list.
const FakeCore = fakeCoreClass(rt);
const core = new FakeCore();
core.nextHandle = 5n;
const board = await stores.Board.create(core);
assert.equal(board.owner.get(), "00000000-0000-0000-0000-000000000000");
assert.deepEqual([...board.blob.get()], []);
assert.equal(board.nickname.get(), null);
assert.equal(board.limit.get(), 0);
const patch = new UndraWriter();
encodePatch(patch, [{ op: "insert", index: 0, item: todo }], t.TodoCodec);
core.deliver(5n, 0, ChangeOp.KeyedPatch, patch.finish());
assert.deepEqual(board.todos.get(), [todo]);
core.deliver(5n, 1, ChangeOp.FullValue, encodeValue(t.UserIdCodec, t.UserId(uuid)));
assert.equal(board.owner.get(), uuid);
assert.deepEqual(core.reports, []);

console.log("ok");

function bytes(text) {
  return new Uint8Array(Buffer.from(text, "hex"));
}
