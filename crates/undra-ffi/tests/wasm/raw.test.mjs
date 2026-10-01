// The wasm ABI (SPEC 7) of the real undra-ffi module, driven directly through WebAssembly with a
// hand-written host: no TypeScript runtime involved. Run with `node --test` after building the
// fixture (see README.md in this directory).
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  Instance,
  Reader,
  Status,
  Writer,
  call,
  fnv1a64,
  ids,
  loadModule,
  parseChangeSet,
  parseReply,
  portReply,
} from "./helpers.mjs";

const module = loadModule();
const fresh = (options) => new Instance(module, options);

const SPEC_EXPORTS = [
  "undra_alloc", "undra_free", "undra_abi_version", "undra_schema_hash", "undra_schema_json", "undra_init",
  "undra_call", "undra_call_sync", "undra_cancel", "undra_stream_credit", "undra_observe", "undra_release",
  "undra_port_reply", "undra_event", "undra_timer_fired", "undra_poll", "undra_snapshot", "undra_restore",
  "undra_buf_free", "undra_stats_json",
];
const SPEC_IMPORTS = ["reply", "changeset", "stream", "port_call", "schedule", "timer_set", "log", "now_ms", "random"];

const i64 = (n) => new Writer().u64(n).done();
const u32 = (n) => new Writer().u32(n).done();
const CALC = "Calculator";

test("the module exports and imports exactly the SPEC 7 surface", () => {
  const exports = WebAssembly.Module.exports(module);
  const fns = exports.filter((e) => e.kind === "function").map((e) => e.name).sort();
  assert.deepEqual(fns, [...SPEC_EXPORTS, "_initialize"].sort());
  assert.deepEqual(exports.filter((e) => e.kind !== "function").map((e) => `${e.kind}:${e.name}`), ["memory:memory"]);
  const imports = WebAssembly.Module.imports(module);
  assert.ok(imports.every((i) => i.module === "undra" && i.kind === "function"));
  assert.deepEqual(imports.map((i) => i.name).sort(), [...SPEC_IMPORTS].sort());
});

test("undra_alloc and undra_free hand out 8-aligned, distinct blocks", () => {
  const core = fresh();
  core.x._initialize();
  const blocks = [0, 1, 7, 100, 65_536].map((n) => [core.x.undra_alloc(n), n]);
  for (const [ptr] of blocks) assert.equal(ptr % 8, 0);
  assert.equal(new Set(blocks.map(([p]) => p)).size, blocks.length);
  for (const [ptr, n] of blocks) core.x.undra_free(ptr, n);
  core.x.undra_free(0, 0); // freeing null is harmless
});

test("undra_alloc traps for a size nothing can satisfy instead of returning 0 (L3)", () => {
  for (const size of [0x7fff_fff9, 0x8000_0000, 0xffff_ffff]) {
    const core = fresh();
    core.x._initialize();
    assert.throws(
      () => core.x.undra_alloc(size),
      (error) => error instanceof WebAssembly.RuntimeError,
      `undra_alloc(${size >>> 0}) must trap, not hand back linear address 0`,
    );
    // The panic hook reported it at level 5 through the host before the trap.
    const fatal = core.logs.filter((l) => l.level === 5);
    assert.equal(fatal.length, 1, JSON.stringify(core.logs));
    assert.match(fatal[0].message, /undra_alloc/);
  }
  // An ordinary size is unaffected, and never 0.
  const core = fresh();
  core.x._initialize();
  assert.notEqual(core.x.undra_alloc(0x1000), 0);
});

test("version, schema hash and schema JSON are consistent and work before undra_init", () => {
  const core = fresh();
  core.x._initialize();
  core.x._initialize(); // idempotent
  assert.equal(core.x.undra_abi_version(), 1);
  const hash = BigInt.asUintN(64, core.x.undra_schema_hash());
  const json = core.takeBuf(core.x.undra_schema_json());
  assert.equal(fnv1a64(json), hash);
  const schema = JSON.parse(new TextDecoder().decode(json));
  assert.ok(JSON.stringify(schema).includes("Calculator"));
  assert.notEqual(hash, 0n);
});

test("undra_init: bad configs are code 2, a good one 0, and repeating it is harmless", () => {
  const core = fresh();
  core.x._initialize();
  for (const bad of [[], [0xff, 0xff, 0xff, 0xff], [...new Writer().str("x").str("sideways").u8(0).u8(0).u8(2).done()]]) {
    assert.equal(core.withBytes(Uint8Array.from(bad), (p, n) => core.x.undra_init(p, n)), 2);
  }
  assert.equal(core.init(), 0);
  assert.equal(core.init(), 0);
  const stats = JSON.parse(new TextDecoder().decode(core.takeBuf(core.x.undra_stats_json())));
  assert.equal(stats.platform, "test");
  assert.equal(stats.mode, "inproc");
  assert.equal(stats.live_handles, 0);
});

test("without undra_init the entry points fail softly", () => {
  const core = fresh();
  core.x._initialize();
  const id = core.callId();
  assert.equal(core.submit(call.fn(ids.fn("version"), id)), 5);
  const reply = core.callSync(call.fn(ids.fn("version"), id));
  assert.equal(reply.status, Status.BadRequest);
  core.x.undra_cancel(1);
  core.observe(0x1_0000_0001n, 0, true);
  const snapshot = core.takeBuf(core.x.undra_snapshot());
  assert.deepEqual([...snapshot], [0, 0, 0, 0, 0, 0, 0, 0]);
  assert.equal(core.withBytes(snapshot, (p, n) => core.x.undra_restore(p, n)), 6);
  core.x.undra_buf_free(0);
});

test("unknown methods and malformed calls are status 5 through both entry points", () => {
  const core = fresh();
  core.init();
  const unknown = 0xdead_beef;
  const id = core.callId();
  const sync = core.callSync(call.fn(unknown, id));
  assert.equal(sync.status, Status.BadRequest);
  assert.equal(sync.callId, id);
  assert.ok(new Reader(sync.body).str().length > 0);
  // undra_call: accepted, reply arrives through the import.
  const id2 = core.callId();
  assert.equal(core.submit(call.fn(unknown, id2)), 0);
  const reply = core.takeReply(id2);
  assert.equal(reply.status, Status.BadRequest);
  // Rejected without a reply: garbage and call id 0.
  assert.equal(core.submit(Uint8Array.of(0xff)), 5);
  assert.equal(core.submit(call.fn(ids.fn("version"), 0)), 5);
  assert.equal(core.replies.length, 0);
  // call_sync of garbage answers status 5 with call id 0.
  assert.deepEqual(core.callSync(Uint8Array.of(0xff, 0xff)).status, Status.BadRequest);
});

test("sync calls, constructors and the typed-error path", () => {
  const core = fresh();
  core.init();
  const version = core.callSync(call.fn(ids.fn("version"), core.callId()));
  assert.equal(version.status, Status.Ok);
  assert.equal(new Reader(version.body).str(), "undra-ffi test core 1");
  const calc = core.construct(CALC, i64(100));
  assert.ok(calc > 0xffff_ffffn, "a handle carries a generation in its high half");
  const add = core.callSync(call.method(calc, ids.method(CALC, "add"), core.callId(), [...i64(2), ...i64(3)]));
  assert.equal(new Reader(add.body).i64(), 105n);
  const fail = core.callSync(call.method(calc, ids.method(CALC, "fail"), core.callId()));
  assert.equal(fail.status, Status.Error);
  assert.equal(new Reader(fail.body).u16(), 0);
  // An async method is refused by call_sync.
  const slow = core.callSync(call.method(calc, ids.method(CALC, "slow_add"), core.callId(), [...i64(1), ...i64(1)]));
  assert.equal(slow.status, Status.BadRequest);
  // Bad arguments name the method.
  const bad = core.callSync(call.method(calc, ids.method(CALC, "add"), core.callId(), [1, 2, 3]));
  assert.equal(bad.status, Status.BadRequest);
  assert.match(new Reader(bad.body).str(), /Calculator\.add/);
  // Released handles are stale.
  core.release(calc);
  const stale = core.callSync(call.method(calc, ids.method(CALC, "add"), core.callId(), [...i64(1), ...i64(1)]));
  assert.equal(stale.status, Status.BadRequest);
});

test("observe delivers the initial change-set before returning; handles cross as lo/hi halves", () => {
  const core = fresh();
  core.init();
  const counter = core.construct("Counter");
  core.observe(counter, 0xffff_ffff, true);
  assert.equal(core.changes.length, 1, "the initial values arrive synchronously");
  let cs = parseChangeSet(core.changes.pop());
  assert.equal(cs.entries.length, 1);
  assert.equal(cs.entries[0].handle, counter);
  assert.equal(new Reader(cs.entries[0].value).u32(), 0);
  const bump = core.callSync(call.method(counter, ids.method("Counter", "bump"), core.callId()));
  assert.equal(bump.status, Status.Ok);
  cs = parseChangeSet(core.changes.pop());
  assert.equal(new Reader(cs.entries[0].value).u32(), 1);
  core.observe(counter, 0xffff_ffff, false);
  core.callSync(call.method(counter, ids.method("Counter", "bump"), core.callId()));
  assert.equal(core.changes.length, 0, "unobserved writes are silent");
  // A bogus handle is a logged no-op.
  core.observe(0x7777_7777_0000_0001n, 0, true);
  const warn = core.logs.find((l) => l.target === "undra::runtime");
  assert.ok(warn && warn.level === 3, JSON.stringify(core.logs));
});

test("snapshot and restore round-trip the stores under the same handle", () => {
  const core = fresh();
  core.init();
  const counter = core.construct("Counter");
  for (let i = 0; i < 3; i++) core.callSync(call.method(counter, ids.method("Counter", "bump"), core.callId()));
  const snapshot = core.takeBuf(core.x.undra_snapshot());
  assert.equal(new Reader(snapshot).u32(), 1);
  core.callSync(call.method(counter, ids.method("Counter", "bump"), core.callId()));
  assert.equal(core.withBytes(snapshot, (p, n) => core.x.undra_restore(p, n)), 0);
  core.observe(counter, 0xffff_ffff, true);
  assert.equal(new Reader(parseChangeSet(core.changes.pop()).entries[0].value).u32(), 3);
  for (const bad of [[0xff, 0xff, 0xff, 0xff], [...snapshot].slice(0, -1), [1]]) {
    assert.equal(core.withBytes(Uint8Array.from(bad), (p, n) => core.x.undra_restore(p, n)), 5);
  }
});

test("async calls run from undra_poll after schedule; timers are host-owned (lo/hi delay)", () => {
  const core = fresh();
  core.init();
  const calc = core.construct(CALC, i64(10));
  const id = core.callId();
  assert.equal(core.submit(call.method(calc, ids.method(CALC, "slow_add"), id, [...i64(1), ...i64(2)])), 0);
  assert.equal(core.takeReply(id), undefined, "no reply before the executor runs");
  assert.ok(core.schedules > 0, "the core asked to be polled");
  core.drain();
  // Background work (undra-query's hydration retry) may arm its own timers, so the
  // sleep's timer is found by its delay rather than assumed to be the only one.
  const slept = core.timers.filter((t) => t.delayMs === 20n);
  assert.equal(slept.length, 1, "the sleep asked the host for a timer");
  core.x.undra_timer_fired(slept[0].timerId);
  core.drain();
  const reply = core.takeReply(id);
  assert.equal(reply.status, Status.Ok);
  assert.equal(new Reader(reply.body).i64(), 13n);

  // A delay above 2^32 ms crosses as two i32 halves.
  core.timers.length = 0;
  const wide = core.callId();
  const ms = (1n << 32n) + 7n;
  assert.equal(core.submit(call.method(calc, ids.method(CALC, "sleep_wide"), wide, i64(ms))), 0);
  core.drain();
  assert.ok(core.timers.some((t) => t.delayMs === ms), "the wide delay crossed as lo/hi");
  // Cancelling answers status 3 exactly once.
  core.x.undra_cancel(wide);
  assert.equal(core.takeReply(wide).status, Status.Cancelled);
  core.x.undra_cancel(wide);
  assert.equal(core.takeReply(wide), undefined);
  // Unknown ids and timers are ignored.
  core.x.undra_cancel(0xffff_0000);
  core.x.undra_timer_fired(0xffff_0000);
});

test("streams honour credit and end", () => {
  const core = fresh();
  core.init();
  const calc = core.construct(CALC, i64(0));
  const id = core.callId();
  assert.equal(core.submit(call.method(calc, ids.method(CALC, "ticks"), id, u32(4))), 0);
  assert.equal(core.takeReply(id).status, Status.StreamOpened);
  core.x.undra_stream_credit(id, 16);
  core.drain();
  const items = core.streams.map(({ payload }) => {
    const r = new Reader(payload);
    return { callId: r.u32(), flag: r.u8(), rest: r.rest() };
  });
  assert.deepEqual(items.map((i) => i.flag), [0, 0, 0, 0, 1]);
  assert.deepEqual(items.slice(0, 4).map((i) => new Reader(i.rest).u32()), [0, 1, 2, 3]);
  assert.ok(items.every((i) => i.callId === id));
});

test("a synchronous port is answered by undra_port_reply inside port_call, which returns 0", () => {
  const core = fresh({
    portCall: (call, self) => {
      assert.equal(call.portId, ids.port("Sum"));
      assert.equal(call.methodId, ids.portMethod("Sum", "add"));
      const r = new Reader(call.args);
      const total = r.u32() + r.u32();
      self.portReplyInto(portReply(call.portCallId, 0, u32(total)));
      return 0;
    },
  });
  core.init();
  const calc = core.construct(CALC, i64(0));
  const reply = core.callSync(call.method(calc, ids.method(CALC, "sum_on_host"), core.callId(), [...u32(20), ...u32(22)]));
  assert.equal(reply.status, Status.Ok);
  assert.equal(new Reader(reply.body).u32(), 42);
});

test("an asynchronous port (return 1) is answered later through undra_port_reply", () => {
  let pending = null;
  const core = fresh({
    portCall: (call) => {
      pending = call;
      return 1;
    },
  });
  core.init();
  const calc = core.construct(CALC, i64(0));
  const id = core.callId();
  assert.equal(core.submit(call.method(calc, ids.method(CALC, "ping_host"), id, u32(7))), 0);
  core.drain();
  assert.ok(pending, "the core called the host port");
  assert.equal(pending.portId, ids.port("Echo"));
  assert.equal(core.takeReply(id), undefined);
  core.portReplyInto(portReply(pending.portCallId, 0, u32(1234)));
  core.drain();
  const reply = core.takeReply(id);
  assert.equal(new Reader(reply.body).u32(), 1234);
  // A late duplicate, an unknown id and garbage are ignored.
  core.portReplyInto(portReply(pending.portCallId, 0, u32(1)));
  core.portReplyInto(portReply(999_999, 0, u32(1)));
  core.portReplyInto(Uint8Array.of(1, 2, 3));
});

test("an async port answered with undra_port_reply inside port_call (returning 0) completes the call", () => {
  const core = fresh({
    portCall: (call, self) => {
      if (call.portId !== ids.port("Echo")) return 2;
      self.portReplyInto(portReply(call.portCallId, 0, u32(new Reader(call.args).u32() + 1000)));
      return 0;
    },
  });
  core.init();
  const calc = core.construct(CALC, i64(0));
  const id = core.callId();
  assert.equal(core.submit(call.method(calc, ids.method(CALC, "ping_host"), id, u32(5))), 0);
  core.drain();
  const reply = core.takeReply(id);
  assert.equal(reply.status, Status.Ok);
  assert.equal(new Reader(reply.body).u32(), 1005);
});

test("a host that answers a different call and returns 0 has not answered this one (L2)", () => {
  // Call A is left pending (return 1). While call B's import runs the host answers A, then
  // returns 0 for B without answering B: B must fail, not wait forever.
  const pending = [];
  const core = fresh({
    portCall: (call, self) => {
      if (call.portId !== ids.port("Echo")) return 2;
      if (pending.length === 0) {
        pending.push(call);
        return 1;
      }
      self.portReplyInto(portReply(pending[0].portCallId, 0, u32(1234)));
      return 0;
    },
  });
  core.init();
  const calc = core.construct(CALC, i64(0));
  const a = core.callId();
  assert.equal(core.submit(call.method(calc, ids.method(CALC, "ping_host"), a, u32(7))), 0);
  core.drain();
  assert.equal(pending.length, 1, "call A reached the host and is pending");
  const b = core.callId();
  assert.equal(core.submit(call.method(calc, ids.method(CALC, "ping_host"), b, u32(8))), 0);
  // B fails as an unavailable port does on wasm (the generated proxy panics, the module traps)
  // and is reported at level 5. Before the fix the lie went undetected and B stayed pending:
  // `drain` returned quietly and no record was written.
  assert.throws(
    () => core.drain(),
    (error) => error instanceof WebAssembly.RuntimeError,
    "call B must fail instead of hanging",
  );
  const fatal = core.logs.filter((l) => l.level === 5);
  assert.equal(fatal.length, 1, JSON.stringify(core.logs));
  assert.match(fatal[0].message, /Echo/);
});

test("Clock, Rng and Log have built-in bindings over now_ms, random and log", () => {
  let now = 1_700_000_000_123.9;
  const core = fresh({ nowMs: () => now }); // the host answers every port call with "unavailable" (2)
  core.init();
  const calc = core.construct(CALC, i64(0));
  const m = (name, args = []) => core.callSync(call.method(calc, ids.method(CALC, name), core.callId(), args));
  assert.equal(new Reader(m("clock_now").body).i64(), 1_700_000_000_123n);
  now = 5;
  const mono1 = new Reader(m("clock_monotonic").body).u64();
  now = 3; // the wall clock goes back: monotonic does not
  const mono2 = new Reader(m("clock_monotonic").body).u64();
  assert.equal(mono1, 5_000_000n);
  assert.ok(mono2 >= mono1);
  const random = m("random_bytes", u32(16));
  const r = new Reader(random.body);
  assert.equal(r.u32(), 16);
  assert.deepEqual([...r.rest()], Array.from({ length: 16 }, (_, i) => (i * 7 + 1) & 0xff));
  assert.deepEqual(core.randomCalls, [16]);
  core.logs.length = 0;
  m("log_line", [...Uint8Array.of(3), ...new Writer().str("hello from rust").done()]);
  assert.deepEqual(
    core.logs.map(({ level, target, message, remaining }) => ({ level, target, message, remaining })),
    [{ level: 3, target: "calculator", message: "hello from rust", remaining: 0 }],
  );
});

test("a host that answers a built-in port overrides it", () => {
  const core = fresh({
    portCall: (call, self) => {
      if (call.portId !== ids.port("Clock")) return 2;
      self.portReplyInto(portReply(call.portCallId, 0, i64(42)));
      return 0;
    },
  });
  core.init();
  const calc = core.construct(CALC, i64(0));
  const reply = core.callSync(call.method(calc, ids.method(CALC, "clock_now"), core.callId()));
  assert.equal(new Reader(reply.body).i64(), 42n);
});

test("a panic logs at level 5 through the host, then traps", () => {
  const core = fresh();
  core.init();
  const calc = core.construct(CALC, i64(0));
  assert.throws(
    () => core.callSync(call.method(calc, ids.method(CALC, "boom"), core.callId())),
    (error) => error instanceof WebAssembly.RuntimeError,
  );
  const fatal = core.logs.filter((l) => l.level === 5);
  assert.equal(fatal.length, 1, `exactly one fatal record: ${JSON.stringify(core.logs)}`);
  assert.equal(fatal[0].target, "undra::panic");
  assert.match(fatal[0].message, /kaboom/);
});
