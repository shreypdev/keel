// Runs the shipped TypeScript runtime's RemoteTransport (compiled, unmodified) against the real
// Rust server. Not part of `cargo test`; see run.sh.
//
//   node ts.mjs <the `serve` example binary> <the compiled @undra/runtime (its dist/ directory)>
import { spawn } from "node:child_process";
import assert from "node:assert/strict";
import net from "node:net";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const bin = process.argv[2];
const dist = process.argv[3];
const {
  ALL_SIGNALS, CallTarget, ChangeOp, UndraCore, UndraSchemaMismatchError, UndraSessionLostError, UndraStore, Signal,
  codecs, decodeValue, encodeValue,
} = await import(pathToFileURL(join(dist, "index.js")).href);

const stderr = [];
/** Starts the `serve` example (on `addr`) and resolves with its process and the JSON line it prints. */
async function startServer(addr) {
  const child = spawn(bin, addr ? [addr] : [], { stdio: ["pipe", "pipe", "pipe"] });
  child.stderr.on("data", (d) => stderr.push(d.toString()));
  const info = await new Promise((resolve, reject) => {
    let buf = "";
    child.stdout.on("data", (d) => {
      buf += d.toString();
      if (buf.includes("\n")) resolve(JSON.parse(buf.split("\n")[0]));
    });
    child.on("error", reject);
  });
  return { child, info };
}
let { child, info } = await startServer();
const schema = BigInt(info.schema);
console.log("server:", info.url, info.schema);

const i32 = (n) => encodeValue(codecs.i32, n);
const cat = (...parts) => Uint8Array.from(parts.flatMap((p) => [...p]));

class CounterStore extends UndraStore {
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
const core = await UndraCore.load({
  mode: "remote",
  url: info.url,
  expectedSchemaHash: schema,
  platform: "node",
  devtools: true,
  shared: false,
  reconnect: false, // this part checks what a dropped connection did before clients reconnected
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
  UndraCore.load({ mode: "remote", url: info.url, expectedSchemaHash: schema ^ 0xffffn, shared: false }),
  (e) => e instanceof UndraSchemaMismatchError && e.expected === (schema ^ 0xffffn) && e.got === schema,
);
console.log("ok  schema mismatch rejected as UndraSchemaMismatchError");

const secondCloses = [];
const second = await UndraCore.load({
  mode: "remote", url: info.url, expectedSchemaHash: schema, shared: false, reconnect: false, onClose: (e) => secondCloses.push(e.message),
});
for (let i = 0; i < 100 && secondCloses.length === 0; i++) await new Promise((r) => setTimeout(r, 20));
assert.ok(secondCloses.length === 1 && secondCloses[0].includes("1013"), JSON.stringify(secondCloses));
console.log("ok  second client closed with 1013:", secondCloses[0]);
second.close();

// The first client is still fine.
assert.equal(decodeValue(codecs.i32, await core.call(target, info.add, i32(1))), 9);

core.close();
for (let i = 0; i < 100 && !stderr.join("").includes("client disconnected"); i++) await new Promise((r) => setTimeout(r, 20));

// ---- reconnecting (ADR-051): through a proxy that can cut connections, against the same server ----
const serverUrl = new URL(info.url);
const proxyConnections = new Set();
const proxy = net.createServer((client) => {
  const upstream = net.connect(Number(serverUrl.port), serverUrl.hostname);
  proxyConnections.add(client).add(upstream);
  client.pipe(upstream);
  upstream.pipe(client);
  for (const sock of [client, upstream]) {
    sock.on("error", () => {});
    sock.on("close", () => { proxyConnections.delete(client); proxyConnections.delete(upstream); client.destroy(); upstream.destroy(); });
  }
});
await new Promise((r) => proxy.listen(0, "127.0.0.1", r));
const dropAll = () => { for (const sock of [...proxyConnections]) sock.destroy(); };
const proxyUrl = `ws://127.0.0.1:${proxy.address().port}`;

const states = [];
const losses = [];
const pc = await UndraCore.load({
  mode: "remote", url: proxyUrl, expectedSchemaHash: schema, platform: "node", devtools: true, shared: false,
  reconnect: { initialDelayMs: 50, maxDelayMs: 200 },
  onConnectionChange: (s) => states.push(s.kind === "reconnecting" ? `reconnecting ${s.attempt}` : s.kind === "closed" ? `closed:${s.reason}` : s.kind),
  onClose: (e) => losses.push(e),
});
const phandle = await pc.construct(info.counter, info.new, i32(5));
const pstore = await CounterStore.create(pc, phandle);
const ptarget = { target: CallTarget.ObjectMethod, handle: phandle };
assert.equal(decodeValue(codecs.i32, await pc.call(ptarget, info.add, i32(3))), 8);
for (let i = 0; i < 50 && pstore.count.peek() !== 8; i++) await new Promise((r) => setTimeout(r, 10));
const hang = pc.call(ptarget, info.ask, i32(1)).then(() => "answered", (e) => e); // the port answers; make it race the drop
dropAll();
const hangOutcome = await hang;
for (let i = 0; i < 200 && states.at(-1) !== "connected"; i++) await new Promise((r) => setTimeout(r, 20));
assert.deepEqual(states.slice(0, 3), ["connecting", "connected", "reconnecting 1"]);
assert.equal(states.at(-1), "connected", JSON.stringify(states));
console.log("ok  a dropped connection was reconnected:", states.join(", "), hangOutcome === "answered" ? "(the call finished first)" : "(the call in flight failed: " + hangOutcome.reason + ")");
// The server kept the object: the same handle still works, with its state, and the mirror was observed again.
assert.equal(decodeValue(codecs.i32, await pc.call(ptarget, info.add, i32(1))), 9);
for (let i = 0; i < 100 && pstore.count.peek() !== 9; i++) await new Promise((r) => setTimeout(r, 10));
assert.equal(pstore.count.peek(), 9);
console.log("ok  the session resumed: same object, same state (9), mirror converged");

// The core is rebuilt by `undra dev`: the server restarts on the same address. The client reconnects, finds a new core and says so.
const address = `${serverUrl.hostname}:${serverUrl.port}`;
child.stdin.end(); // the server shuts down (1001) ...
await new Promise((r) => child.on("exit", r));
for (let i = 0; i < 100 && !states.some((s) => s.startsWith("reconnecting")) ; i++) await new Promise((r) => setTimeout(r, 20));
({ child, info } = await startServer(address)); // ... and a new one starts
for (let i = 0; i < 300 && losses.length === 0; i++) await new Promise((r) => setTimeout(r, 20));
assert.ok(losses.length === 1 && losses[0] instanceof UndraSessionLostError, JSON.stringify(losses.map(String)));
assert.equal(states.at(-1), "closed:sessionLost");
assert.ok(pc.closed);
console.log("ok  a restarted server: the client reports a lost session, once:", states.slice(-3).join(", "));
proxy.close();
dropAll();
child.stdin.end();
if (child.exitCode === null) await new Promise((r) => child.on("exit", r));
console.log("all TypeScript interop checks passed");
if (process.env.SHOW_LOG) console.log(stderr.join(""));
