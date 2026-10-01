// The shipped TypeScript runtime's RemoteTransport (compiled, unmodified) as the *app* client of a
// dev server that has a devtools page attached (ADR-054): the page (a plain WebSocket here, speaking
// the `[tag][body]` protocol of `undra_transport::devtools::proto`) time-travels the core, and the
// app's mirror must converge on the core's state through the change-sets the restore emits.
// Not part of `cargo test`; see run.sh.
//
//   node ts-devtools.mjs <the `serve` example binary> <the compiled @undra/runtime (its dist/ directory)>
import { spawn } from "node:child_process";
import assert from "node:assert/strict";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const bin = process.argv[2];
const dist = process.argv[3];
const {
  ALL_SIGNALS, CallTarget, ChangeOp, UndraCore, UndraStore, Signal, codecs, decodeValue, encodeValue,
} = await import(pathToFileURL(join(dist, "index.js")).href);

const TOKEN = "interopdevtoolstoken0123456789";
const stderr = [];
const child = spawn(bin, [], { stdio: ["pipe", "pipe", "pipe"], env: { ...process.env, UNDRA_SERVE_DEVTOOLS: TOKEN } });
child.stderr.on("data", (d) => stderr.push(d.toString()));
const info = await new Promise((resolve, reject) => {
  let buf = "";
  child.stdout.on("data", (d) => {
    buf += d.toString();
    if (buf.includes("\n")) resolve(JSON.parse(buf.split("\n")[0]));
  });
  child.on("error", reject);
});
const schema = BigInt(info.schema);
const i32 = (n) => encodeValue(codecs.i32, n);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(what, cond, ms = 5000) {
  for (let t = 0; t < ms; t += 10) {
    const got = cond();
    if (got) return got;
    await sleep(10);
  }
  assert.fail(`timed out waiting for ${what}; the server said: ${stderr.join("").slice(-600)}`);
}

class CounterStore extends UndraStore {
  count = new Signal(-1);
  constructor(core, handle) {
    super(core, handle);
    this._signals = [this.count];
  }
  static async create(core, handle) {
    const store = new CounterStore(core, handle);
    await core.observe(handle, ALL_SIGNALS, true);
    return store;
  }
  _apply(signalId, op, value) {
    if (signalId === 0 && op === ChangeOp.FullValue) this.count._set(decodeValue(codecs.i32, value));
  }
}

const notices = [];
const core = await UndraCore.load({
  mode: "remote", url: info.url, expectedSchemaHash: schema, platform: "node", devtools: true, shared: false, reconnect: false,
  onDevNotice: (m) => notices.push(m),
});
const handle = await core.construct(info.counter, info.new, i32(5));
const store = await CounterStore.create(core, handle);
const target = { target: CallTarget.ObjectMethod, handle };
const add = async (n) => decodeValue(codecs.i32, await core.call(target, info.add, i32(n)));
await until("the mirror to show 5", () => store.count.peek() === 5);

// A page with the wrong token is refused; the right one is let in (Node's global WebSocket).
const wsUrl = (token) => `${info.url.replace(/\/$/, "")}/devtools/ws?token=${token}`;
const refused = await new Promise((resolve) => {
  const ws = new WebSocket(wsUrl("wrongwrongwrongwrongwrong"));
  let opened = false;
  ws.onopen = () => { opened = true; };
  ws.onclose = () => resolve(!opened);
  ws.onerror = () => {};
});
assert.ok(refused, "a page without the token never opens");
console.log("ok  a page with the wrong token is refused");

const page = new WebSocket(wsUrl(TOKEN));
page.binaryType = "arraybuffer";
const messages = [];
page.onmessage = (e) => messages.push(new DataView(e.data));
await until("the page to open", () => page.readyState === WebSocket.OPEN);
const TAG = { welcome: 1, step: 4, traveled: 9 };
const steps = () => messages.filter((m) => m.getUint8(0) === TAG.step).map((m) => m.getUint32(1, true));
await until("the welcome", () => messages.some((m) => m.getUint8(0) === TAG.welcome));
const first = (await until("the first step", () => steps()[0]));
console.log(`ok  the page attached; step ${first} is the state it attached to (5)`);

// The app writes; the page's ring records it; the app's mirror follows as it always did.
assert.equal(await add(3), 8);
await until("the mirror to show 8", () => store.count.peek() === 8);
await until("a second step", () => steps().some((s) => s > first));

// Time travel: Restore { request_id, step } = [1][u32][u32], little endian.
const restore = new DataView(new ArrayBuffer(9));
restore.setUint8(0, 1);
restore.setUint32(1, 77, true);
restore.setUint32(5, first, true);
page.send(restore.buffer);
const traveled = await until("the answer", () => messages.find((m) => m.getUint8(0) === TAG.traveled));
assert.equal(traveled.getUint32(1, true), 77);
assert.equal(traveled.getUint8(5), 1, "the restore succeeded");
assert.equal(traveled.getUint32(10, true), 0, "no store was dropped");

// The app converged through its own session: the mirror equals the core.
await until("the mirror to show 5 again", () => store.count.peek() === 5);
assert.equal(await add(0), 5, "the core really is at the step");
assert.equal(store.count.peek(), 5, "and so is the mirror");
await until("the dev notice", () => notices.some((m) => m === `time travel: step ${first}`));
console.log("ok  time travel: the TypeScript mirror converged on the core (5), notice:", notices.at(-1));

// The app keeps working afterwards, and the mirror keeps following.
assert.equal(await add(2), 7);
await until("the mirror to show 7", () => store.count.peek() === 7);
console.log("ok  the app carries on: mirror 7 = core 7");

page.close();
await sleep(100);
assert.equal(await add(1), 8);
await until("the mirror to show 8", () => store.count.peek() === 8);
console.log("ok  after the page left the app is unchanged");
core.close();
child.stdin.end();
if (child.exitCode === null) await new Promise((r) => child.on("exit", r));
console.log("all TypeScript devtools interop checks passed");
