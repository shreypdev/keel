// The real undra-ffi wasm module driven by the real TypeScript runtime (`UndraCore` over
// `WasmMainTransport`, mode "wasm-main", and over `WasmWorkerTransport` with the core on a real
// `worker_threads` Worker, mode "wasm-worker"): the acceptance run for SPEC 7 and SPEC 17.1.
// Needs the TypeScript runtime built: run.sh builds it fresh into a scratch directory and sets
// UNDRA_TS_DIST; by hand, `npm ci && npx tsc -p tsconfig.build.json` in runtimes/ts/@undra/runtime
// (a dist/ older than its sources is refused, so this can never test a stale build).
import assert from "node:assert/strict";
import { readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { after, test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { Worker, isMainThread } from "node:worker_threads";
import { Instance, ids, loadModule } from "./helpers.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const tsRoot = resolve(here, "../../../../runtimes/ts/@undra/runtime");
const dist = process.env.UNDRA_TS_DIST ?? resolve(tsRoot, "dist/index.js");
if (!process.env.UNDRA_TS_DIST) refuseStaleDist(dist, join(tsRoot, "src"));

/** Throws when `dist` is missing or older than the newest TypeScript source: a stale build would pass or fail for the wrong reasons. */
function refuseStaleDist(dist, src) {
  let built;
  try {
    built = statSync(dist).mtimeMs;
  } catch {
    throw new Error(`${dist} does not exist: run crates/undra-ffi/tests/wasm/run.sh, or build the TypeScript runtime (npm ci && npx tsc -p tsconfig.build.json)`);
  }
  const newest = (dir) =>
    readdirSync(dir, { withFileTypes: true }).reduce(
      (latest, entry) =>
        Math.max(latest, entry.isDirectory() ? newest(join(dir, entry.name)) : entry.name.endsWith(".ts") ? statSync(join(dir, entry.name)).mtimeMs : 0),
      0,
    );
  if (newest(src) > built) {
    throw new Error(`${dist} is older than the TypeScript sources: rebuild it (run.sh does) instead of testing a stale dist`);
  }
}
const K = await import(pathToFileURL(dist).href);
const {
  ALL_SIGNALS,
  CallTarget,
  UndraCore,
  UndraModeError,
  UndraReplyError,
  UndraRestoreError,
  UndraSchemaMismatchError,
  UndraTransportError,
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
const SCHEMA_HASH = BigInt.asUintN(64, scratch.x.undra_schema_hash());

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

async function boot({ log = [], adapters = {}, ports, onClose, logLevel, drains } = {}) {
  const core = await UndraCore.load({
    mode: "wasm-main",
    wasm: module,
    expectedSchemaHash: SCHEMA_HASH,
    platform: "test",
    ...(logLevel !== undefined && { logLevel }),
    shared: false,
    adapters: { http: null, log: { log: (level, target, message) => log.push({ level, target, message }) }, ...adapters },
    ...(ports && { ports }),
    ...(onClose && { onClose }),
    // `drains` collects the mirror's scheduled drains instead of running them: only an explicit flush applies what the core delivered on its own.
    ...(drains && { mirror: { schedule: (fn) => drains.push(fn) } }),
  });
  opened.push(core);
  return core;
}

// ----- wasm-worker mode on a real worker thread -----------------------------------------------------

/** The worker script of the TypeScript runtime, built next to `dist/index.js`. */
const WORKER_MODULE = pathToFileURL(join(dirname(dist), "worker.js")).href;
/**
 * What the worker thread runs. `dist/worker.js` starts serving by itself only under a real
 * `WorkerGlobalScope`, which a Node worker thread does not have, so the script calls `runWorker` on
 * `parentPort` (a Node `MessagePort` is an `EventTarget`: it already has the shape of a worker scope).
 */
const WORKER_SOURCE = `
const { parentPort } = require("node:worker_threads");
import(${JSON.stringify(WORKER_MODULE)}).then(
  ({ runWorker }) => runWorker(parentPort),
  (error) => { console.error(error); process.exit(1); },
);
`;

const threads = [];
after(() => {
  for (const thread of threads) void thread.terminate();
});

/**
 * A Node `Worker` in the shape of the runtime's `WorkerLike` (the DOM `Worker` API): `addEventListener`
 * handlers get `{ data }` for messages and `{ message }` for errors, `terminate` stops the thread.
 */
function workerLike(thread) {
  const wrapped = new Map();
  const wrap = (type, fn) => {
    if (type === "message") return (data) => fn({ data });
    if (type === "error") return (error) => fn({ message: error?.message ?? String(error), error });
    return () => fn({});
  };
  return {
    addEventListener(type, fn) {
      const listener = wrap(type, fn);
      wrapped.set(fn, listener);
      thread.on(type, listener);
    },
    removeEventListener(type, fn) {
      const listener = wrapped.get(fn);
      if (listener) thread.off(type, listener);
      wrapped.delete(fn);
    },
    postMessage: (message, transfer) => thread.postMessage(message, transfer),
    terminate: () => void thread.terminate(),
  };
}

/** Loads the fixture core on a worker thread: `UndraCore.load({ mode: "wasm-worker" })`. */
async function bootWorker({ log = [], adapters = {}, ports, onClose, onError, drains } = {}) {
  const thread = new Worker(WORKER_SOURCE, { eval: true });
  threads.push(thread);
  const core = await UndraCore.load({
    mode: "wasm-worker",
    worker: workerLike(thread),
    wasm: module,
    expectedSchemaHash: SCHEMA_HASH,
    platform: "test",
    shared: false,
    adapters: { http: null, log: { log: (level, target, message) => log.push({ level, target, message }) }, ...adapters },
    ...(ports && { ports }),
    ...(onClose && { onClose }),
    ...(onError && { onError }),
    ...(drains && { mirror: { schedule: (fn) => drains.push(fn) } }),
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

test("load refuses a core built from another schema before undra_init runs", async () => {
  const failure = await UndraCore.load({
    mode: "wasm-main",
    wasm: module,
    expectedSchemaHash: SCHEMA_HASH ^ 1n,
    shared: false,
    adapters: { http: null },
  }).catch((e) => e);
  assert.ok(failure instanceof UndraSchemaMismatchError, String(failure));
  assert.equal(failure.got, SCHEMA_HASH);
});

test("sync and async calls, typed errors and bad requests", async () => {
  const core = await boot();
  assert.equal(decodeValue(codecs.string, core.callSync(FREE, ids.fn("version"), new Uint8Array(0))), "undra-ffi test core 1");
  const calc = await calculator(core);
  assert.ok(calc > 0xffff_ffffn);
  assert.equal(decodeValue(codecs.i64, callSync(core, calc, "add", concat(i64(2), i64(3)))), 105n);
  // The async method runs on undra_poll; its timer is a setTimeout that calls undra_timer_fired.
  assert.equal(decodeValue(codecs.i64, await call(core, calc, "slow_add", concat(i64(2), i64(3)))), 105n);
  await assert.rejects(call(core, calc, "fail"), (e) => e instanceof UndraReplyError && e.status === ReplyStatus.Error);
  await assert.rejects(
    core.call(FREE, 0xdead_beef, new Uint8Array(0)),
    (e) => e instanceof UndraReplyError && e.status === ReplyStatus.BadRequest,
  );
  assert.throws(
    () => core.callSync(FREE, 0xdead_beef, new Uint8Array(0)),
    (e) => e instanceof UndraReplyError && e.status === ReplyStatus.BadRequest,
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
  assert.ok(log.some((l) => l.level === 3 && l.target === "undra::runtime"), JSON.stringify(log));
  assert.ok(!log.some((l) => l.level < 3), JSON.stringify(log));
});

test("a panic in the core logs at level 5, then the transport reports a trap and closes", async () => {
  const log = [];
  let closed = null;
  const core = await boot({ log, onClose: (error) => (closed = error) });
  const calc = await calculator(core);
  assert.throws(
    () => callSync(core, calc, "boom"),
    (e) => e instanceof UndraTransportError && e.reason === "trap",
  );
  const fatal = log.filter((l) => l.level === 5);
  assert.ok(fatal.length >= 1, JSON.stringify(log));
  assert.match(fatal[0].message, /kaboom/);
  assert.equal(fatal[0].target, "undra::panic");
  await macrotask();
  assert.ok(closed instanceof UndraTransportError, "the handler heard that the core died");
  assert.throws(() => callSync(core, calc, "add", concat(i64(1), i64(1))), UndraTransportError);
});

// ----- randomness never degrades silently (ADR-049 decision 2.5, gap PO-11) -------------------------

test("a host without a random source makes Rng.fill unavailable: the core fails loudly (E0062) after an ERROR naming the cause, never zeros", async () => {
  const log = [];
  let closed = null;
  const core = await boot({
    log,
    adapters: { rng: { fill: () => {
      throw new Error("no CSPRNG on this host");
    } } },
    onClose: (error) => (closed = error),
  });
  const calc = await calculator(core);
  assert.throws(
    () => callSync(core, calc, "random_bytes", u32(16)),
    (e) => e instanceof UndraTransportError && e.reason === "trap",
  );
  const cause = log.find((l) => l.level === 4 && l.target === "undra::rng");
  assert.ok(cause, JSON.stringify(log));
  assert.match(cause.message, /no cryptographic random source/);
  // The runtime also says what the import threw.
  assert.ok(log.some((l) => l.level === 4 && /no CSPRNG on this host/.test(l.message)), JSON.stringify(log));
  const fatal = log.filter((l) => l.level === 5 && l.target === "undra::panic");
  assert.equal(fatal.length, 1, JSON.stringify(log));
  assert.match(fatal[0].message, /E0062/);
  assert.ok(log.indexOf(cause) < log.indexOf(fatal[0]));
  await macrotask();
  assert.ok(closed instanceof UndraTransportError);
});

test("UndraCore.load refuses both wasm modes without WebCrypto, before anything is instantiated", async () => {
  const saved = Object.getOwnPropertyDescriptor(globalThis, "crypto");
  Object.defineProperty(globalThis, "crypto", { value: undefined, configurable: true, writable: true });
  try {
    for (const mode of ["wasm-main", "wasm-worker"]) {
      let spawned = false;
      const failure = await UndraCore.load({
        mode,
        wasm: module,
        expectedSchemaHash: SCHEMA_HASH,
        shared: false,
        worker: () => {
          spawned = true;
          throw new Error("no worker should be spawned");
        },
        adapters: { http: null },
      }).catch((e) => e);
      assert.ok(failure instanceof UndraTransportError, `${mode}: ${String(failure)}`);
      assert.equal(failure.reason, "unsupported");
      assert.match(failure.message, /^WebCrypto is required/);
      assert.equal(spawned, false);
    }
  } finally {
    Object.defineProperty(globalThis, "crypto", saved);
  }
});

// ----- wasm-worker mode: the core on a real worker thread (gap PO-4) ---------------------------------

/** Waits until `probe()` is truthy (the worker's output reaches the main thread through message events). */
async function until(what, probe, timeoutMs = 5_000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = probe();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

test("wasm-worker: a real core on a real worker thread loads, answers calls and refuses callSync", async () => {
  const core = await bootWorker();
  assert.equal(core.mode, "wasm-worker");
  assert.equal(core.hello.schemaHash, SCHEMA_HASH);
  const calc = await calculator(core);
  assert.equal(decodeValue(codecs.i64, await call(core, calc, "add", concat(i64(2), i64(3)))), 105n);
  assert.equal(decodeValue(codecs.i64, await call(core, calc, "slow_add", concat(i64(2), i64(3)))), 105n);
  assert.throws(() => core.callSync(method(calc), ids.method(CALC, "add"), concat(i64(1), i64(1))), UndraModeError);
  const stats = await core.stats();
  assert.equal(stats.core.platform, "test");
});

test("wasm-worker: Clock, Rng and Log are answered inside the worker, the core does not trap", async () => {
  const log = [];
  const closed = [];
  const core = await bootWorker({ log, onClose: (error) => closed.push(error) });
  const calc = await calculator(core);

  // Clock.now_ms: the worker's own Date.now.
  const before = Date.now();
  const now = Number(decodeValue(codecs.i64, await call(core, calc, "clock_now")));
  const after = Date.now();
  assert.ok(now >= before - 5_000 && now <= after + 5_000, `clock_now ${now} is not within a few seconds of ${before}`);

  // Clock.monotonic_ns: never goes backwards.
  const first = decodeValue(codecs.u64, await call(core, calc, "clock_monotonic"));
  const second = decodeValue(codecs.u64, await call(core, calc, "clock_monotonic"));
  assert.ok(first > 0n && second >= first, `monotonic ${first} then ${second}`);

  // Rng.fill: the worker's crypto.getRandomValues.
  const a = decodeValue(codecs.bytes, await call(core, calc, "random_bytes", u32(16)));
  const b = decodeValue(codecs.bytes, await call(core, calc, "random_bytes", u32(16)));
  assert.equal(a.length, 16);
  assert.equal(b.length, 16);
  assert.ok(a.some((byte) => byte !== 0), "16 random bytes that are all zero");
  assert.notDeepEqual([...a], [...b], "two draws gave the same 16 bytes");
  assert.deepEqual([...decodeValue(codecs.bytes, await call(core, calc, "random_bytes", u32(0)))], []);

  // Log.log: the record crosses to the main thread's Log adapter, with its level, target and text.
  await call(core, calc, "log_line", concat(Uint8Array.of(3), encodeValue(codecs.string, "from the worker")));
  await until("the log record", () => log.some((l) => l.target === "calculator"));
  assert.deepEqual(log.filter((l) => l.target === "calculator"), [{ level: 3, target: "calculator", message: "from the worker" }]);

  assert.equal(core.closed, false);
  assert.deepEqual(closed, [], "nothing closed the core");
  // And it still answers.
  assert.equal(decodeValue(codecs.i64, await call(core, calc, "add", concat(i64(1), i64(1)))), 102n);
});

test("wasm-worker: an async host port still round-trips through the main thread", async () => {
  const askedOn = [];
  const echo = { sync: false, methods: { [ids.portMethod("Echo", "ping")]: async (args) => {
    askedOn.push(isMainThread);
    await macrotask();
    return u32(new DataView(args.buffer, args.byteOffset, args.byteLength).getUint32(0, true) + 1000);
  } } };
  const core = await bootWorker({ ports: { [ids.port("Echo")]: echo } });
  const calc = await calculator(core);
  assert.equal(decodeValue(codecs.u32, await call(core, calc, "ping_host", u32(7))), 1007);
  assert.equal(decodeValue(codecs.u32, await call(core, calc, "ping_host", u32(1))), 1001);
  assert.deepEqual(askedOn, [true, true], "the port implementation ran on the main thread");
});

test("wasm-worker: a host Clock or Rng does not cross to the worker (the built-in bindings serve the core)", async () => {
  let asked = 0;
  const core = await bootWorker({
    adapters: { clock: { nowMs: () => 1_234_567, monotonicNs: () => 9n }, rng: { fill: (out) => out.fill(0xab) } },
    ports: {
      [ids.port("Clock")]: clockPort({ nowMs: () => (asked++, 42), monotonicNs: () => 43n }),
      [ids.port("Rng")]: rngPort({ fill: (out) => out.fill(1) }),
    },
  });
  const calc = await calculator(core);
  const now = Number(decodeValue(codecs.i64, await call(core, calc, "clock_now")));
  assert.ok(Math.abs(now - Date.now()) < 5_000, `the worker's own clock: ${now}`);
  assert.equal(decodeValue(codecs.bytes, await call(core, calc, "random_bytes", u32(4))).length, 4);
  assert.notDeepEqual([...decodeValue(codecs.bytes, await call(core, calc, "random_bytes", u32(8)))], new Array(8).fill(1));
  assert.equal(asked, 0, "the main thread's Clock was never asked");
});

test("wasm-worker: a custom sync port cannot be served; the core's call fails loudly and the reason is logged", async () => {
  const log = [];
  const closed = [];
  const sum = { sync: true, methods: { [ids.portMethod("Sum", "add")]: (args) => {
    const r = new DataView(args.buffer, args.byteOffset, args.byteLength);
    return u32(r.getUint32(0, true) + r.getUint32(4, true));
  } } };
  const core = await bootWorker({ log, ports: { [ids.port("Sum")]: sum }, onClose: (error) => closed.push(error) });
  const calc = await calculator(core);
  // `Sum.add` returns a plain u32 and the core calls it synchronously: the worker cannot wait for the main thread,
  // the port is unavailable to it, and the infallible proxy panics (a trap on wasm). Typed, not silent.
  const failure = await call(core, calc, "sum_on_host", concat(u32(20), u32(22))).then(() => undefined, (e) => e);
  assert.ok(failure instanceof UndraTransportError, String(failure));
  assert.equal(failure.reason, "trap");
  assert.ok(log.some((l) => l.level === 5 && l.target === "undra::panic"), `the panic reached the Log adapter: ${JSON.stringify(log)}`);
  await until("the core to close", () => core.closed);
  assert.equal(closed.length, 1);
  // The main thread said why, once.
  await until("the warning", () => log.some((l) => l.target === "undra::worker" && l.level === 3));
  const warnings = log.filter((l) => l.target === "undra::worker" && l.level === 3);
  assert.equal(warnings.length, 1);
  assert.match(warnings[0].message, /wasm-worker/);
  assert.match(warnings[0].message, /wasm-main/);
});

// ----- snapshot and restore on UndraCore (gap PA-5), in both wasm modes -------------------------------

const BUMP = ids.method("Counter", "bump");
const COUNTER_NEW = ids.method("Counter", "new");

/** A `Counter` store observed through the mirror: `seen` is every value of its signal, in order. */
async function observedCounter(core) {
  const counter = await core.construct(ids.type("Counter"), COUNTER_NEW, new Uint8Array(0));
  const seen = [];
  core.mirror.register(counter, (_signalId, _op, value) => seen.push(decodeValue(codecs.u32, value)));
  await core.observe(counter, ALL_SIGNALS, true);
  const bump = () => core.call(method(counter), BUMP, new Uint8Array(0));
  return { counter, seen, bump };
}

/** Runs `body(boot)` in each wasm mode. The mirror's frame scheduler never runs by itself, so only a `flush` can apply what the core delivered on its own (a restore). */
function inEachMode(name, body) {
  for (const mode of ["wasm-main", "wasm-worker"]) {
    test(`${mode}: ${name}`, async () => {
      const drains = [];
      const bootMode = (options = {}) => (mode === "wasm-main" ? boot({ ...options, drains }) : bootWorker({ ...options, drains }));
      await body(mode, bootMode);
    });
  }
}

inEachMode("snapshot, mutate, restore: the same handle shows the snapshot's value as soon as restore resolves", async (_mode, bootMode) => {
  const core = await bootMode();
  const { counter, seen, bump } = await observedCounter(core);
  await bump();
  await bump();
  assert.equal(seen.at(-1), 2);
  const snapshot = await core.snapshot();
  assert.ok(snapshot instanceof Uint8Array && snapshot.length > 0, "a non-empty snapshot");
  for (let i = 0; i < 3; i++) await bump();
  assert.equal(seen.at(-1), 5);
  await core.restore(snapshot);
  assert.equal(seen.at(-1), 2, "the restored value had reached the store when restore() resolved");
  // The handle is the same one and is live.
  await bump();
  assert.equal(seen.at(-1), 3);
  assert.equal(core.mirror.has(counter), true);
  // restore() did not consume or change the caller's bytes.
  assert.ok(snapshot.length > 0);
  await core.restore(snapshot);
  assert.equal(seen.at(-1), 2);
});

inEachMode("a snapshot that is not one is refused with UndraRestoreError, and the core is unchanged and usable", async (_mode, bootMode) => {
  const core = await bootMode();
  const { seen, bump } = await observedCounter(core);
  await bump();
  const junk = Uint8Array.from({ length: 16 }, (_, i) => 0xf0 + i);
  const refusal = await core.restore(junk).then(() => undefined, (e) => e);
  assert.ok(refusal instanceof UndraRestoreError, String(refusal));
  assert.equal(refusal.code, 5);
  assert.equal(refusal.kind, "restore");
  assert.equal(core.closed, false);
  assert.deepEqual(seen, [0, 1], "nothing was delivered for the refused bytes");
  await bump();
  assert.equal(seen.at(-1), 2);
  // An empty snapshot is junk too.
  await assert.rejects(core.restore(new Uint8Array(0)), UndraRestoreError);
});

inEachMode("a call in flight across a restore is cancelled by the core, and a non-store handle goes stale", async (_mode, bootMode) => {
  const core = await bootMode();
  const calc = await calculator(core);
  const never = call(core, calc, "never");
  never.catch(() => {});
  await macrotask();
  await core.restore(await core.snapshot());
  const cancelled = await never.then(() => undefined, (e) => e);
  assert.ok(cancelled instanceof UndraReplyError, String(cancelled));
  assert.equal(cancelled.status, ReplyStatus.Cancelled);
  // The calculator is not a store, so the restore invalidated its handle.
  const stale = await call(core, calc, "add", concat(i64(1), i64(1))).then(() => undefined, (e) => e);
  assert.ok(stale instanceof UndraReplyError, String(stale));
  assert.equal(stale.status, ReplyStatus.BadRequest);
  assert.equal(core.closed, false);
});

inEachMode("a closed core rejects snapshot and restore with UndraTransportError('closed')", async (_mode, bootMode) => {
  const core = await bootMode();
  core.close();
  for (const run of [() => core.snapshot(), () => core.restore(new Uint8Array(8))]) {
    const error = await run().then(() => undefined, (e) => e);
    assert.ok(error instanceof UndraTransportError, String(error));
    assert.equal(error.reason, "closed");
  }
});

test("a snapshot taken in one mode restores into a core of the other, and into a fresh core of the same", async () => {
  const worker = await bootWorker();
  const { counter, bump } = await observedCounter(worker);
  await bump();
  await bump();
  await bump();
  const snapshot = await worker.snapshot();

  for (const fresh of [await boot(), await bootWorker()]) {
    await fresh.restore(snapshot);
    // The restore re-issued the same handle in the new core: observe it through the mirror.
    const seen = [];
    fresh.mirror.register(counter, (_signalId, _op, value) => seen.push(decodeValue(codecs.u32, value)));
    await fresh.observe(counter, ALL_SIGNALS, true);
    assert.deepEqual(seen, [3], `the fresh ${fresh.mode} core shows the snapshot's value`);
    await fresh.call(method(counter), BUMP, new Uint8Array(0));
    await until("the bump", () => seen.at(-1) === 4);
  }
});
