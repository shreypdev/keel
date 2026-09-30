// Execution test of the `stdlib` golden case: the standard library is left out of the generated
// package, and everything that refers to it runs on the runtime's own types and codecs
// (ADR-024). Usage: node stdlib.mjs <package>.

import assert from "node:assert/strict";

import { bytes, fakeCoreClass, hex, setup } from "./lib.mjs";

const { rt, types, errors, objects, stores, ports, UndraIds } = await setup(process.argv[2]);
const { encodeValue, decodeValue, codecs, ReplyStatus } = rt;
const FakeCore = fakeCoreClass(rt);

// ----- nothing of the standard library is generated --------------------------------------------

for (const name of ["HttpMethod", "Header", "HttpRequest", "HttpResponse", "NetKind", "AppState"]) {
  assert.equal(types[`${name}Codec`], undefined, `types declares ${name}`);
}
for (const name of ["HttpError", "FsError"]) {
  assert.equal(errors[name], undefined, `errors declares ${name}`);
  assert.equal(errors[`${name}Codec`], undefined, `errors declares ${name}Codec`);
}
for (const name of ["clockPortImpl", "httpPortImpl", "kvPortImpl", "fsPortImpl", "ConnectivityEvents", "LifecycleEvents"]) {
  assert.equal(ports[name], undefined, `ports declares ${name}`);
}
assert.deepEqual(Object.keys(UndraIds.Ports), ["Uploader"]);
// The app's own type may share the name of a standard port.
assert.equal(typeof types.ConnectivityCodec, "object");

// ----- records that hold standard types use the runtime's codecs ---------------------------------

const request = {
  method: "post",
  url: "/u",
  headers: [{ name: "a", value: "b" }],
  body: new Uint8Array([1]),
  timeoutMs: 5000,
};
const response = { status: 204, headers: [], body: new Uint8Array(0) };
const endpoint = {
  name: "x",
  request,
  fallback: response,
  accepted: ["get", "post"],
  extra: [{ name: "c", value: "d" }],
};
const endpointBytes =
  "01000000" + "78" + // name
  hex(encodeValue(rt.HttpRequestCodec, request)) + // request, as the runtime writes it
  "01" + hex(encodeValue(rt.HttpResponseCodec, response)) + // fallback: Some(response)
  "02000000" + "0000" + "0100" + // accepted: Get, Post
  "01000000" + "01000000" + "63" + "01000000" + "64"; // extra
assert.equal(hex(encodeValue(types.EndpointCodec, endpoint)), endpointBytes);
assert.deepEqual(decodeValue(types.EndpointCodec, bytes(endpointBytes)), endpoint);
assert.throws(
  () => decodeValue(types.EndpointCodec, bytes(endpointBytes.replace("02000000" + "0000" + "0100", "02000000" + "0000" + "0900"))),
  (e) => e.code === "invalid_tag" && e.detail.ty === "HttpMethod",
);

const connectivity = { online: true, kind: "wired", app: "background" };
assert.equal(hex(encodeValue(types.ConnectivityCodec, connectivity)), "01" + "0200" + "01" + "0200");
assert.deepEqual(decodeValue(types.ConnectivityCodec, bytes("01" + "0200" + "01" + "0200")), connectivity);

// ----- a generated error wraps the runtime's ---------------------------------------------------

const wrapped = new errors.SyncError.Http(new rt.HttpError.Timeout());
assert.equal(wrapped.kind, "http");
assert.equal(wrapped.message, "the request timed out");
assert.ok(wrapped.cause instanceof rt.HttpError.Timeout);
assert.equal(hex(encodeValue(errors.SyncErrorCodec, wrapped)), "0100" + "0100");
const disk = decodeValue(errors.SyncErrorCodec, bytes("0200" + "0200" + "02000000" + "6d6d" + "01000000" + "78"));
assert.ok(disk instanceof errors.SyncError.Disk);
assert.ok(disk.value0 instanceof rt.FsError.Io);
assert.equal(disk.message, "disk failure at x");

// ----- calls fail with the runtime's typed errors ------------------------------------------------

const replyError = (codec, value) => new rt.UndraReplyError(ReplyStatus.Error, encodeValue(codec, value));

{
  const core = new FakeCore();
  const syncer = await objects.Syncer.create(core);

  core.replies.push(encodeValue(rt.HttpResponseCodec, response));
  assert.deepEqual(await syncer.send(request), response);
  assert.equal(core.calls[0].args, hex(encodeValue(rt.HttpRequestCodec, request)));

  core.replies.push(replyError(rt.HttpErrorCodec, new rt.HttpError.Network("down")));
  await assert.rejects(
    syncer.send(request),
    (e) => e instanceof rt.HttpError && e instanceof rt.HttpError.Network && e.value === "down",
  );
  // Anything but an error reply passes through untouched.
  const panic = new rt.UndraReplyError(ReplyStatus.Panic, new Uint8Array(0));
  core.replies.push(panic);
  await assert.rejects(syncer.send(request), (e) => e === panic);

  core.replies.push(replyError(rt.FsErrorCodec, new rt.FsError.Denied()));
  await assert.rejects(syncer.save("/tmp/x"), (e) => e instanceof rt.FsError.Denied);

  core.replies.push(replyError(errors.SyncErrorCodec, new errors.SyncError.Offline()));
  await assert.rejects(syncer.sync(), (e) => e instanceof errors.SyncError.Offline);

  // A stream decodes standard items and surfaces the typed failure while iterating.
  core.streams.push([encodeValue(rt.HttpResponseCodec, response), replyError(rt.HttpErrorCodec, new rt.HttpError.Cancelled())]);
  const seen = [];
  await assert.rejects(
    (async () => {
      for await (const item of syncer.follow(endpoint)) seen.push(item);
    })(),
    (e) => e instanceof rt.HttpError.Cancelled,
  );
  assert.deepEqual(seen, [response]);
}

// ----- a store with signals of standard types ---------------------------------------------------

{
  const core = new FakeCore();
  core.nextHandle = 9n;
  const link = await stores.Link.create(core);
  // Placeholders until the first change-set.
  assert.equal(link.state.get(), "active");
  assert.equal(link.kind.get(), "wifi");
  assert.equal(link.last.get(), null);
  assert.equal(link.failure.get(), null);
  assert.deepEqual(link.pending.get(), []);

  core.deliver(9n, 0, rt.ChangeOp.FullValue, encodeValue(rt.AppStateCodec, "inactive"));
  core.deliver(9n, 1, rt.ChangeOp.FullValue, encodeValue(rt.NetKindCodec, "none"));
  core.deliver(9n, 2, rt.ChangeOp.FullValue, encodeValue(codecs.option(rt.HttpResponseCodec), response));
  core.deliver(9n, 3, rt.ChangeOp.FullValue, encodeValue(codecs.option(rt.HttpErrorCodec), new rt.HttpError.InvalidUrl("x")));
  core.deliver(9n, 4, rt.ChangeOp.FullValue, encodeValue(codecs.vec(rt.HttpRequestCodec), [request]));
  assert.equal(link.state.get(), "inactive");
  assert.equal(link.kind.get(), "none");
  assert.deepEqual(link.last.get(), response);
  assert.ok(link.failure.get() instanceof rt.HttpError.InvalidUrl);
  assert.deepEqual(link.pending.get(), [request]);
}

// ----- a port of the app's own that fails with a standard error -------------------------------------

{
  const upload = ports.uploaderPortImpl({
    upload: async (req) => {
      if (req.url === "/cancel") throw new rt.HttpError.Cancelled();
      return response;
    },
  }).methods[UndraIds.Ports.Uploader.upload];
  assert.equal(hex(await upload(encodeValue(rt.HttpRequestCodec, request))), hex(encodeValue(rt.HttpResponseCodec, response)));
  await assert.rejects(
    upload(encodeValue(rt.HttpRequestCodec, { ...request, url: "/cancel" })),
    (e) => e instanceof rt.UndraPortError && hex(e.body) === "0200",
  );
}
