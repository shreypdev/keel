// Runs the shipped TypeScript runtime's RemoteTransport (compiled, unmodified) against the real
// Rust server. Not part of `cargo test`; see run.sh.
//
//   node ts.mjs <the `serve` example binary> <the compiled @keel/runtime (its dist/ directory)>
import { spawn } from "node:child_process";
import assert from "node:assert/strict";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const bin = process.argv[2];
const dist = process.argv[3];
const {
  ALL_SIGNALS, CallTarget, ChangeOp, KeelCore, KeelSchemaMismatchError, KeelStore, Signal,
  codecs, decodeValue, encodeValue,
} = await import(pathToFileURL(join(dist, "index.js")).href);

const child = spawn(bin, [], { stdio: ["pipe", "pipe", "pipe"] });
const stderr = [];
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
console.log("server:", info.url, info.schema);

const i32 = (n) => encodeValue(codecs.i32, n);
const cat = (...parts) => Uint8Array.from(parts.flatMap((p) => [...p]));

class CounterStore extends KeelStore {
  count = new Signal(0);
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

const closes = [];
const echoed = [];
const core = await KeelCore.load({
  mode: "remote",
  url: info.url,
  expectedSchemaHash: schema,
  platform: "node",
  devtools: true,
  shared: false,
  onClose: (e) => closes.push(e.message),
  ports: {
    [info.echoPort]: {
      sync: false,
      methods: { [info.echoMethod]: async (args) => { const x = decodeValue(codecs.i32, args); echoed.push(x); return i32(x + 1); } },
    },
  },
});
console.log("ok  handshake");

assert.equal(decodeValue(codecs.i32, await core.call(CallTarget.FreeFunction, info.sum, cat(i32(2), i32(40)))), 42);
console.log("ok  free function call");

const handle = await core.construct(info.counter, info.new, i32(5));
const store = await CounterStore.create(core, handle);
assert.equal(store.count.peek(), 5);
const target = { target: CallTarget.ObjectMethod, handle };
assert.equal(decodeValue(codecs.i32, await core.call(target, info.add, i32(3))), 8);
for (let i = 0; i < 50 && store.count.peek() !== 8; i++) await new Promise((r) => setTimeout(r, 10));
assert.equal(store.count.peek(), 8);
console.log("ok  constructor, method, observe, change-set applied to the mirror");

assert.equal(decodeValue(codecs.i32, await core.call(target, info.ask, i32(41))), 42);
assert.deepEqual(echoed, [41]);
console.log("ok  port call answered by the node adapter");

await assert.rejects(
  KeelCore.load({ mode: "remote", url: info.url, expectedSchemaHash: schema ^ 0xffffn, shared: false }),
  (e) => e instanceof KeelSchemaMismatchError && e.expected === (schema ^ 0xffffn) && e.got === schema,
);
console.log("ok  schema mismatch rejected as KeelSchemaMismatchError");

const secondCloses = [];
const second = await KeelCore.load({
  mode: "remote", url: info.url, expectedSchemaHash: schema, shared: false, onClose: (e) => secondCloses.push(e.message),
});
for (let i = 0; i < 100 && secondCloses.length === 0; i++) await new Promise((r) => setTimeout(r, 20));
assert.ok(secondCloses.length === 1 && secondCloses[0].includes("1013"), JSON.stringify(secondCloses));
console.log("ok  second client closed with 1013:", secondCloses[0]);
second.close();

// The first client is still fine.
assert.equal(decodeValue(codecs.i32, await core.call(target, info.add, i32(1))), 9);

child.stdin.end(); // the server shuts down
for (let i = 0; i < 200 && closes.length === 0; i++) await new Promise((r) => setTimeout(r, 20));
assert.ok(closes.length === 1 && closes[0].includes("1001"), JSON.stringify(closes));
console.log("ok  server shutdown reported as a close with 1001:", closes[0]);
if (child.exitCode === null) await new Promise((r) => child.on("exit", r));
console.log("all TypeScript interop checks passed");
if (process.env.SHOW_LOG) console.log(stderr.join(""));
