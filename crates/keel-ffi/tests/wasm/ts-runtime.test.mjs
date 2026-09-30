// The real keel-ffi wasm module driven by the real TypeScript runtime (`KeelCore` over
// `WasmMainTransport`, mode "wasm-main"): the acceptance run for SPEC 7 and SPEC 17.1.
// Needs the TypeScript runtime built: `npm ci && npx tsc -p tsconfig.build.json` in
// runtimes/ts/@keel/runtime (or KEEL_TS_DIST=/path/to/dist/index.js).
import assert from "node:assert/strict";
import { dirname, resolve } from "node:path";
import { after, test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { Instance, ids, loadModule } from "./helpers.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const dist = process.env.KEEL_TS_DIST ?? resolve(here, "../../../../runtimes/ts/@keel/runtime/dist/index.js");
const K = await import(pathToFileURL(dist).href);
const {
  ALL_SIGNALS,
  CallTarget,
  KeelCore,
  KeelReplyError,
  KeelSchemaMismatchError,
  KeelTransportError,
  ReplyStatus,
  clockPort,
  codecs,
  decodeValue,
  encodeValue,
  rngPort,
} = K;

const module = loadModule();
// The schema hash the "generated bindings" would carry: read it off a scratch instance.
const scratch = new Instance(module);
scratch.x._initialize();
const SCHEMA_HASH = BigInt.asUintN(64, scratch.x.keel_schema_hash());

const FREE = CallTarget.FreeFunction;
const method = (handle) => ({ target: CallTarget.ObjectMethod, handle });
const i64 = (n) => encodeValue(codecs.i64, BigInt(n));
const u32 = (n) => encodeValue(codecs.u32, n);
const concat = (...parts) => Uint8Array.from(parts.flatMap((p) => [...p]));
const CALC = "Calculator";
const macrotask = () => new Promise((resolve) => setTimeout(resolve, 0));

const opened = [];
after(() => {
  for (const core of opened) core.close();
});

async function boot({ log = [], adapters = {}, ports, onClose, logLevel } = {}) {
  const core = await KeelCore.load({
    mode: "wasm-main",
    wasm: module,
    expectedSchemaHash: SCHEMA_HASH,
    platform: "test",
    ...(logLevel !== undefined && { logLevel }),
    shared: false,
    adapters: { http: null, log: { log: (level, target, message) => log.push({ level, target, message }) }, ...adapters },
    ...(ports && { ports }),
    ...(onClose && { onClose }),
  });
  opened.push(core);
  return core;
}

const calculator = async (core, base = 100) => core.construct(ids.type(CALC), ids.method(CALC, "new"), i64(base));
const call = (core, handle, name, args = new Uint8Array(0)) => core.call(method(handle), ids.method(CALC, name), args);
const callSync = (core, handle, name, args = new Uint8Array(0)) => core.callSync(method(handle), ids.method(CALC, name), args);

test("load: handshake, hello and core statistics", async () => {
  const core = await boot();
  assert.equal(core.mode, "wasm-main");
  assert.equal(core.hello.schemaHash, SCHEMA_HASH);
  assert.equal(core.hello.mode, "inproc");
  const stats = await core.stats();
  assert.equal(stats.core.platform, "test");
  assert.equal(stats.core.mode, "inproc");
});

test("load refuses a core built from another schema before keel_init runs", async () => {
  const failure = await KeelCore.load({
    mode: "wasm-main",
    wasm: module,
    expectedSchemaHash: SCHEMA_HASH ^ 1n,
    shared: false,
    adapters: { http: null },
  }).catch((e) => e);
  assert.ok(failure instanceof KeelSchemaMismatchError, String(failure));
  assert.equal(failure.got, SCHEMA_HASH);
});

test("sync and async calls, typed errors and bad requests", async () => {
  const core = await boot();
  assert.equal(decodeValue(codecs.string, core.callSync(FREE, ids.fn("version"), new Uint8Array(0))), "keel-ffi test core 1");
  const calc = await calculator(core);
  assert.ok(calc > 0xffff_ffffn);
  assert.equal(decodeValue(codecs.i64, callSync(core, calc, "add", concat(i64(2), i64(3)))), 105n);
  // The async method runs on keel_poll; its timer is a setTimeout that calls keel_timer_fired.
  assert.equal(decodeValue(codecs.i64, await call(core, calc, "slow_add", concat(i64(2), i64(3)))), 105n);
  await assert.rejects(call(core, calc, "fail"), (e) => e instanceof KeelReplyError && e.status === ReplyStatus.Error);
  await assert.rejects(
    core.call(FREE, 0xdead_beef, new Uint8Array(0)),
    (e) => e instanceof KeelReplyError && e.status === ReplyStatus.BadRequest,
  );
  assert.throws(
    () => core.callSync(FREE, 0xdead_beef, new Uint8Array(0)),
    (e) => e instanceof KeelReplyError && e.status === ReplyStatus.BadRequest,
  );
  // Several timers at once resolve in due order.
  const order = [];
  await Promise.all(
    [30, 10, 20].map((ms) => call(core, calc, "sleep_ms", u32(ms)).then(() => order.push(ms))),
  );
  assert.deepEqual(order, [10, 20, 30]);
});

test("cancelling a call with an AbortSignal cancels it in the core", async () => {
  const core = await boot();
  const calc = await calculator(core);
  const controller = new AbortController();
  const pending = core.call(method(calc), ids.method(CALC, "never"), new Uint8Array(0), controller.signal);
  await macrotask();
  controller.abort();
  await assert.rejects(pending);
  await macrotask();
  const stats = await core.stats();
  assert.equal(stats.core.crossings.cancelled, 1);
});

test("streams deliver items in order and end", async () => {
  const core = await boot();
  const calc = await calculator(core);
  const seen = [];
  for await (const item of core.stream(method(calc), ids.method(CALC, "ticks"), u32(40))) seen.push(decodeValue(codecs.u32, item));
  assert.deepEqual(seen, Array.from({ length: 40 }, (_, i) => i));
});

test("a store is observed through the mirror: initial values, then updates", async () => {
  const core = await boot();
  const counter = await core.construct(ids.type("Counter"), ids.method("Counter", "new"), new Uint8Array(0));
  const seen = [];
  core.mirror.register(counter, (signalId, op, value) => seen.push([signalId, decodeValue(codecs.u32, value)]));
  await core.observe(counter, ALL_SIGNALS, true);
  assert.deepEqual(seen, [[0, 0]], "the initial change-set was applied before observe resolved");
  core.callSync(method(counter), ids.method("Counter", "bump"), new Uint8Array(0));
  await macrotask();
  assert.deepEqual(seen, [[0, 0], [0, 1]]);
  core.release(counter);
  const stats = await core.stats();
  assert.equal(stats.core.live_handles, 0);
});

test("host ports: a sync port and an async port, both registered through registerPort", async () => {
  const sum = { sync: true, methods: { [ids.portMethod("Sum", "add")]: (args) => {
    const r = new DataView(args.buffer, args.byteOffset, args.byteLength);
    return u32(r.getUint32(0, true) + r.getUint32(4, true));
  } } };
  const echo = { sync: false, methods: { [ids.portMethod("Echo", "ping")]: async (args) => {
    await macrotask();
    return u32(new DataView(args.buffer, args.byteOffset, args.byteLength).getUint32(0, true) + 1000);
  } } };
  const core = await boot({ ports: { [ids.port("Sum")]: sum, [ids.port("Echo")]: echo } });
  const calc = await calculator(core);
  assert.equal(decodeValue(codecs.u32, callSync(core, calc, "sum_on_host", concat(u32(20), u32(22)))), 42);
  assert.equal(decodeValue(codecs.u32, await call(core, calc, "ping_host", u32(7))), 1007);
});

test("Clock, Rng and Log are built in over now_ms, random and log; adapters and ports override them", async () => {
  const log = [];
  const core = await boot({
    log,
    adapters: {
      clock: { nowMs: () => 1_234_567, monotonicNs: () => 9n },
      rng: { fill: (out) => out.fill(0xab) },
    },
  });
  const calc = await calculator(core);
  // The adapters back the imports, so the built-in ports read them.
  assert.equal(decodeValue(codecs.i64, callSync(core, calc, "clock_now")), 1_234_567n);
  assert.equal(decodeValue(codecs.u64, callSync(core, calc, "clock_monotonic")), 1_234_567_000_000n);
  assert.deepEqual([...decodeValue(codecs.bytes, callSync(core, calc, "random_bytes", u32(4)))], [0xab, 0xab, 0xab, 0xab]);
  // Log: a record from Rust code reaches the adapter.
  callSync(core, calc, "log_line", concat(Uint8Array.of(3), encodeValue(codecs.string, "hello")));
  assert.deepEqual(log.filter((l) => l.target === "calculator"), [{ level: 3, target: "calculator", message: "hello" }]);
  // Registering the port (host side) wins over the built-in.
  core.registerPort(ids.port("Clock"), clockPort({ nowMs: () => 42, monotonicNs: () => 43n }));
  assert.equal(decodeValue(codecs.i64, callSync(core, calc, "clock_now")), 42n);
  assert.equal(decodeValue(codecs.u64, callSync(core, calc, "clock_monotonic")), 43n);
  core.registerPort(ids.port("Rng"), rngPort({ fill: (out) => out.fill(1) }));
  assert.deepEqual([...decodeValue(codecs.bytes, callSync(core, calc, "random_bytes", u32(2)))], [1, 1]);
});

test("the core's own log records reach the log adapter, filtered by the configured level", async () => {
  const log = [];
  const core = await boot({ log, logLevel: 3 });
  // An unknown handle is a level-3 warning from the runtime; a stale release is level 1 (filtered).
  await core.observe(0x7777_7777_0000_0001n, 0, true).catch(() => {});
  core.release(0x7777_7777_0000_0001n);
  await macrotask();
  assert.ok(log.some((l) => l.level === 3 && l.target === "keel::runtime"), JSON.stringify(log));
  assert.ok(!log.some((l) => l.level < 3), JSON.stringify(log));
});

test("a panic in the core logs at level 5, then the transport reports a trap and closes", async () => {
  const log = [];
  let closed = null;
  const core = await boot({ log, onClose: (error) => (closed = error) });
  const calc = await calculator(core);
  assert.throws(
    () => callSync(core, calc, "boom"),
    (e) => e instanceof KeelTransportError && e.reason === "trap",
  );
  const fatal = log.filter((l) => l.level === 5);
  assert.ok(fatal.length >= 1, JSON.stringify(log));
  assert.match(fatal[0].message, /kaboom/);
  assert.equal(fatal[0].target, "keel::panic");
  await macrotask();
  assert.ok(closed instanceof KeelTransportError, "the handler heard that the core died");
  assert.throws(() => callSync(core, calc, "add", concat(i64(1), i64(1))), KeelTransportError);
});
