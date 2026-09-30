// Execution test of the `errors` golden case: message rendering, reserved field names and the
// module cycle between `types.js` and `errors.js`. Usage: node errors.mjs <package> [types-first|errors-first].

import assert from "node:assert/strict";

import { bytes, hex, loader, setup } from "./lib.mjs";

const pkg = process.argv[2];
const order = process.argv[3] ?? "types-first";
// Whichever module of the cycle is imported first, every codec must be initialised by the time it
// is used.
const load = loader(pkg);
if (order === "errors-first") await load("dist/errors.js");
const { rt, types, errors } = await setup(pkg);
const { encodeValue, decodeValue } = rt;

// ----- messages -------------------------------------------------------------------------

assert.equal(new errors.HttpError.Timeout().message, "timed out");
assert.equal(new errors.HttpError.Status(503).message, "status 503");
assert.equal(new errors.HttpError.Network("boom").message, "network error: boom");
assert.equal(new errors.TodoError.EmptyTitle().message, "title cannot be empty");
assert.equal(new errors.TodoError.NotFound("id-1").message, 'todo "id-1" not found, {sorry}');
assert.equal(new errors.TodoError.Range(9, 3).message, "9 is not below 3");
assert.equal(new errors.TodoError.Http(new errors.HttpError.Status(404)).message, "status 404");

// Fields that clash with `Error` members or reserved words are renamed.
const storage = new errors.TodoError.Storage("disk", 7, "full");
assert.equal(storage.message, "storage failure 7: disk ($full)");
assert.equal(storage.reason, "disk");
assert.equal(storage.code, 7);
assert.equal(storage.message_, "full");
assert.equal(new errors.TodoError.Detail("x").default_, "x");
assert.equal(new errors.TodoError.Detail("x").message, "detail x");

// ----- wire ------------------------------------------------------------------------------

assert.equal(
  hex(encodeValue(errors.TodoErrorCodec, storage)),
  "0300" + "04000000" + "6469736b" + "07000000" + "04000000" + "66756c6c",
);
const back = decodeValue(errors.TodoErrorCodec, encodeValue(errors.TodoErrorCodec, storage));
assert.ok(back instanceof errors.TodoError.Storage);
assert.equal(back.message, storage.message);
assert.equal(hex(encodeValue(errors.HttpErrorCodec, new errors.HttpError.Cancelled())), "0300");
assert.throws(() => decodeValue(errors.HttpErrorCodec, bytes("0900")), (e) => e.name === "WireError" && e.code === "invalid_tag");

// A record holding errors, holding a record ...
const payload = {
  text: "outer",
  failure: new errors.HttpError.Status(500),
  history: [new errors.Boxed.Holding({ text: "inner", failure: null, history: [] })],
};
const payloadBytes = encodeValue(types.PayloadCodec, payload);
assert.equal(
  hex(payloadBytes),
  "05000000" + "6f75746572" + // text
    "01" + "0100" + "f401" + // failure: Some(Status(500))
    "01000000" + "0000" + "05000000" + "696e6e6572" + "00" + "00000000", // history: [Holding(Payload)]
);
const decoded = decodeValue(types.PayloadCodec, payloadBytes);
assert.equal(decoded.text, "outer");
assert.ok(decoded.failure instanceof errors.HttpError.Status);
assert.equal(decoded.failure.value, 500);
assert.ok(decoded.history[0] instanceof errors.Boxed.Holding);
assert.equal(decoded.history[0].value.text, "inner");
assert.equal(decoded.history[0].value.failure, null);

console.log("ok");
