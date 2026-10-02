// Execution test of the `callbacks` golden case (ADR-041). Usage: node callbacks.mjs <package>.

import assert from "node:assert/strict";

import { fakeCoreClass, setup } from "./lib.mjs";

const { rt, objects, errors, callbacks, UndraIds } = await setup(process.argv[2]);
const { UndraWriter, UndraReplyError, ReplyStatus, UndraPortError, codecs, decodeValue, encodeValue } = rt;
const FakeCore = fakeCoreClass(rt);
const ids = UndraIds.Callbacks.UploadListener;

/** The arguments of a call of the core into `instance`. */
function args(instance, write = () => {}) {
  const w = new UndraWriter();
  w.writeU64(instance);
  write(w);
  return w.finish();
}

const core = new FakeCore();
const registry = rt.callbacks(core);
const heard = [];
const listener = {
  progress: (sent, total) => heard.push(["progress", sent, total]),
  finished: (name) => heard.push(["finished", name]),
  answer: true,
  confirmReplace(name, signal) {
    heard.push(["confirm", name]);
    if (this.answer === "typed") return Promise.reject(new errors.PromptError.Declined());
    if (this.answer === "bug") return Promise.reject(new TypeError("broken"));
    if (this.answer === "wait") {
      return new Promise((_, reject) => signal.addEventListener("abort", () => reject(signal.reason)));
    }
    return Promise.resolve(this.answer);
  },
};

// A constructor with an optional callback: none here.
const uploader = await objects.Uploader.create(null, core);
assert.equal(core.constructed[0].args, "00");

// A method lends the listener: its instance handle is written and the bridge is registered once.
core.replies.push(encodeValue(codecs.u32, 7));
assert.equal(await uploader.upload("a.txt", listener), 7);
const instance = decodeValue(codecs.u64, Buffer.from(core.calls.at(-1).args.slice(-16), "hex"));
assert.equal(instance, 1n);
assert.equal(registry.count(listener), 1);
const bridge = core.ports.get(ids.portId);
assert.equal(bridge.sync, false);
assert.equal(bridge.name, "UploadListener");

// The same listener twice is one instance counted twice; the core gives one back with `__release`, in order.
core.replies.push(encodeValue(codecs.u32, 8));
await uploader.upload("b.txt", listener);
assert.equal(registry.count(listener), 2);
assert.equal(registry.liveCount, 1);

// Fire-and-forget calls only queue (nothing runs inside the core's callback); the drain runs them in order, and a
// `coalesce` method keeps only the newest per instance.
const noReply = bridge.methods[ids.progress](args(instance, (w) => (w.writeU64(1n), w.writeU64(10n))), 0);
bridge.methods[ids.finished](args(instance, (w) => w.writeStr("a.txt")), 0);
bridge.methods[ids.progress](args(instance, (w) => (w.writeU64(10n), w.writeU64(10n))), 0);
assert.deepEqual(heard, []);
assert.equal(core.drain(), 2, "two invocations delivered, the superseded progress not");
assert.deepEqual(heard, [["finished", "a.txt"], ["progress", 10n, 10n]]);
let settled = false;
noReply.then(() => (settled = true));
await Promise.resolve();
assert.equal(settled, false, "a call with port call id 0 is never answered");

// An async method answers through the port reply: its value, its typed error (status 1), or "unavailable" after
// reporting anything else.
heard.length = 0;
const yes = bridge.methods[ids.confirmReplace](args(instance, (w) => w.writeStr("a.txt")), 11);
core.drain();
assert.equal(decodeValue(codecs.bool, await yes), true);
listener.answer = "typed";
const typed = bridge.methods[ids.confirmReplace](args(instance, (w) => w.writeStr("b")), 12);
core.drain();
await assert.rejects(typed, (e) => e instanceof UndraPortError && decodeValue(errors.PromptErrorCodec, e.body) instanceof errors.PromptError.Declined);
listener.answer = "bug";
const bug = bridge.methods[ids.confirmReplace](args(instance, (w) => w.writeStr("c")), 13);
core.drain();
await assert.rejects(bug, (e) => !(e instanceof Error), "a quiet unavailable: reported here, not by the dispatcher");
assert.equal(core.reports.at(-1).operation, "UploadListener.confirmReplace");
assert.ok(core.reports.at(-1).error instanceof TypeError);

// `__cancel` drops an invocation that has not started, and aborts the signal of one that has.
listener.answer = "wait";
heard.length = 0;
const dropped = bridge.methods[ids.confirmReplace](args(instance, (w) => w.writeStr("d")), 14);
bridge.methods[ids.cancelCall](args(instance, (w) => w.writeU32(14)), 0);
assert.equal(core.drain(), 0);
await assert.rejects(dropped);
assert.deepEqual(heard, [], "the cancelled invocation never ran");
const running = bridge.methods[ids.confirmReplace](args(instance, (w) => w.writeStr("e")), 15);
core.drain();
bridge.methods[ids.cancelCall](args(instance, (w) => w.writeU32(15)), 0);
await assert.rejects(running);
const reports = core.reports.length;
await Promise.resolve();
assert.equal(core.reports.length, reports, "the abort of a cancelled call is not a failure to report");

// `__release` takes effect when the queue reaches it; the entry goes with the last reference.
bridge.methods[ids.releaseInstance](args(instance), 0);
assert.equal(registry.count(listener), 2);
core.drain();
assert.equal(registry.count(listener), 1);
bridge.methods[ids.releaseInstance](args(instance), 0);
core.drain();
assert.equal(registry.liveCount, 0);
// A call into an instance the registry no longer holds answers "unavailable".
await assert.rejects(bridge.methods[ids.confirmReplace](args(instance, (w) => w.writeStr("f")), 16));

// A refused call (status 5) transfers nothing: the generated code gave the reference back.
core.replies.push(new UndraReplyError(ReplyStatus.BadRequest, new Uint8Array(0)));
await assert.rejects(uploader.watch(listener), (e) => e instanceof rt.UndraCallError.Refused);
assert.equal(registry.liveCount, 0);
// A call that reached the core and failed otherwise leaves the reference with the core.
core.replies.push(new UndraReplyError(ReplyStatus.Cancelled, new Uint8Array(0)));
await assert.rejects(uploader.watch(listener), (e) => e instanceof rt.UndraCallError.CancelledByCore);
assert.equal(registry.count(listener), 1);
// A watch returned by a method that lends a callback is adopted.
core.replies.push(encodeValue(codecs.u64, 70n));
const watch = await uploader.watch(listener);
assert.ok(watch instanceof objects.Watch);
assert.equal(registry.count(listener), 2);

// A command that lends reports its failure and gives the reference back when it never reached the core.
const closed = new FakeCore();
closed.call = () => Promise.reject(new rt.UndraTransportError("closed", "the core is closed"));
const elsewhere = await objects.Uploader.create(null, closed);
await elsewhere.notify(listener);
assert.equal(rt.callbacks(closed).liveCount, 0);
assert.equal(closed.reports.at(-1).operation, "Uploader.notify");

// A background interface runs from a microtask, without the drain.
const tokens = [];
const provider = {
  token: async (account) => `token-${account}`,
  refreshed: (account) => tokens.push(account),
};
await uploader.setProvider(provider);
const providerIds = UndraIds.Callbacks.TokenProvider;
const providerBridge = core.ports.get(providerIds.portId);
const providerInstance = rt.callbacks(core).lend(provider, callbacks.TokenProviderCallback);
rt.callbacks(core).giveBack(providerInstance);
providerBridge.methods[providerIds.refreshed](args(providerInstance, (w) => w.writeStr("me")), 0);
const token = providerBridge.methods[providerIds.token](args(providerInstance, (w) => w.writeStr("me")), 21);
assert.equal(core.queuedCalls.length, 0, "not through the mirror");
assert.equal(decodeValue(codecs.string, await token), "token-me");
assert.deepEqual(tokens, ["me"]);

// The weak wrapper forwards while its target lives, then does nothing and answers unavailable.
let alive = true;
const RealWeakRef = globalThis.WeakRef;
globalThis.WeakRef = class {
  #target;
  constructor(target) {
    this.#target = target;
  }
  deref() {
    return alive ? this.#target : undefined;
  }
};
const weak = callbacks.weakUploadListener(listener);
globalThis.WeakRef = RealWeakRef;
heard.length = 0;
listener.answer = true;
weak.finished("x");
assert.equal(await weak.confirmReplace("y", new AbortController().signal), true);
alive = false;
weak.finished("z");
await assert.rejects(weak.confirmReplace("w", new AbortController().signal));
assert.deepEqual(heard, [["finished", "x"], ["confirm", "y"]]);

// A free function lends too.
core.replies.push(encodeValue(codecs.u32, 3));
assert.equal(await objects.withListener(listener, core), 3);

console.log("ok");
