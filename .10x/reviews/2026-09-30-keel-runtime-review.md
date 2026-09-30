# keel-runtime adversarial review (2026-09-30)

Scope: all of `crates/keel-runtime/src` (runtime, executor, ctx, object_table, ports, timer,
blocking, host, dispatch, lazy, guard, ext, stats, log, testing, object, config), read line by line
against SPEC 5, 6, 16.2, `docs/runtime-internals.md`, ADR-018..021, and the call sites in
keel-signals (`StoreCell::observe`, `commit_slots`), keel-query (InitHook, DispatchLayer, extension)
and keel-ffi (`session::start`). The read started at `5a887de`. `54b3d14` (the signals re-review R2
fix, which wraps `Runtime::observe` in a txn) landed during the review, so every repro was re-run
at `cb953c8`, and line numbers refer to `cb953c8`. Every finding marked CONFIRMED was reproduced in
an external crate (`/private/tmp/keel-rt-review/tests/{review,abba,inithook,restore_r2}.rs`, which
depends on the workspace by path; nothing in the repo was touched). THEORY gives the exact code
path. Baseline: `cargo test -p keel-runtime` green (223 passed, 3 soak tests ignored). Severity: no
Critical, 1 High, 3 Medium, 9 Low, 10 Notes.

## Attacks that failed (verified sound)

- **Lock graph.** Edges: core -> {objects, calls, tasks, ready queue, ports.pending/bindings,
  events.subs, timers.state, signals slot/value locks, store delivery lock}; delivery(S) -> {GLOBAL,
  `Host::change_set`/`log`}. Leaf locks never call out (wakers taken out first, `MutexGuard::unlocked`
  in the timer thread, callbacks cloned out of `Events`). The only cycle is M2 below.
- **Executor.** `wait_batch` checks and waits under the queue lock and `push_ready` pushes under it,
  so the unlocked `notify_one` cannot be lost. `begin_poll` clears `queued` before the poll, so a
  wake during the poll re-queues. Duplicate ids in one batch are impossible (dedup flag). Stale
  `TaskId`s are rejected by serial. Spawn/wake/cancel from inside a poll touch no held lock.
- **Call lifecycle.** `call_id` 0 and in-flight duplicates are refused under the core lock. Cancel,
  `finish_call` and `task_panicked` all gate on `calls.remove`, and all run under the core lock, so
  exactly one terminal reply wins. A stream item cannot follow a cancel (same lock). Credit
  saturates, credit for unknown/ended ids is ignored, End/Error need no credit. `call_sync` refuses
  async-shaped methods from metadata before the dispatcher runs (layers excepted, documented).
- **Ports.** `abandon` vs `complete` race is serialized by `pending`; either the reply is delivered
  or discarded, never both. A reply from inside `Host::port_call` is accepted (registered first).
- **Panics (R6).** Dispatchers, polls, subscribers, restorers, drops and host callbacks are
  guarded; a panicking task replies status 2 once and poisons the receiver. Restore fuzz (20,000
  garbage and mutated snapshots, T7): 0 panics, failed restores left the table unchanged.
- **Determinism (R12).** Wall clock and threads only in timer/blocking/core-thread code; no HashMap
  iteration order reaches the host.

## High

### H1. Restore rolls a slot's generation back: stale host handles alias new objects (CONFIRMED)
`object_table.rs:304` (`insert_at`: `slot.generation = handle.generation()`), `runtime.rs:1662-1682`,
`object_table.rs:187-192`, `:467-479`.
What: `clear()` bumps every slot, then `insert_at` sets each restored slot back to the snapshot's
generation, which can be lower than generations already issued at that index. The floor
(`max snapshot generation + 1`) only protects vacant and fresh slots. The next release of the restored
object re-issues a generation the host may still hold. A snapshot carrying generation `u32::MAX`
saturates the floor, and the first release wraps to 1 (T8).
Repro (T1): A=(0,1); snapshot; release A; B=(0,2) (host keeps B's wrapper); restore -> A at (0,1), B
stale (correct); release A; new C = (0,2) == B. The stale `b.get()` returns C's value (3). The host
finalising B's wrapper (`keel_release(b)`) releases C; C's own calls then fail with status 5.
Restoring into a fresh runtime has the same hole for slots the old table had cycled past the
snapshot's max generation (THEORY: slot 5 at gen 7 in the old core, fresh core starts it at the floor).
Fix: keep a per-slot high-water generation that `insert_at` never lowers. `release` jumps to
`max(hwm, gen) + 1` (skip 0). For fresh-runtime restores, carry the table's high-water mark in the
snapshot (wire change, needs an ADR) or re-issue with a floor above anything the host could hold.

## Medium

### M1. Observe/restore entries bypass the store's delivery lock and overtake newer commits (CONFIRMED)
`runtime.rs:1086-1101`, `:681-690` (`next_txn_id()` taken after `cell.observe` returns),
`:1684-1700`; keel-signals `store.rs:359-433` (no delivery lock, `last_txn` not updated) vs `:525-577`.
What: `Runtime::observe` builds entries with `StoreCell::observe` (no delivery lock), then allocates
a txn id and calls `Host::change_set` outside the lock. A commit of the same store on another thread
can claim the slot, get a smaller txn id and deliver the new value before the observe entries,
which carry the old value. That breaks SPEC 3.5 / ADR-020 ("for one store ... whichever thread
commits"). The keel-signals review found the `dirty` race safe at its layer; the ordering fails one
layer up. Restore Phase 3 has the same shape. The `54b3d14` txn does not help: the writer's
transaction lives on another thread.
Repro (T2): store {x: i32, g: Gate}; host observes ALL on thread O; `g`'s encoder blocks after `x=1`
was encoded; an embedder thread (`ctx.enter()`, allowed by `write_allowed`) sets x=2 and commits
(txn 1, x=2); O resumes and delivers txn 2, x=1. Core x=2, host mirror x=1 until the next write.
Fix: give keel-signals an `observe_and_deliver(signal_id, on, sink)` that holds the delivery lock
from encoding through the sink, allocates the txn id under it and updates `last_txn`. The runtime
(observe and restore Phase 3, per store) delivers through it. Consider tightening `write_allowed`
(runtime.rs:103) to "holds this runtime's core lock or no runtime is current".

### M2. E_REENTRANT is blind to the delivery lock: core <-> delivery(S) ABBA deadlock (CONFIRMED)
`runtime.rs:612-631` (`HELD` tracks only the core lock), `runtime.rs:70-75` (the sink runs under
delivery(S)), keel-signals `store.rs:525`.
What: an off-core commit (embedder thread, or a pool worker in release builds) holds delivery(S)
while `Host::change_set` runs on that thread. A host that calls back into the runtime from that
callback is not refused, because the thread does not hold the core. It blocks on the core, while
the core, inside a dispatch that writes S, blocks on delivery(S). SPEC 5.1 promises detection
(`E_REENTRANT`); this path hangs forever instead.
Repro (T5, threaded `Runtime::new`): thread W sets `count` (observed) -> sink -> host sleeps, then
calls `rt.observe`; meanwhile a host thread `call_sync(add)`. Both hang (5 s watchdog fires).
Fix: `RuntimeSink::deliver` (and every host callback) sets a thread-local "in host callback" flag;
`enter_core` refuses with `Reentrant` when it is set, whatever lock the thread holds.

### M3. Restore leaves in-flight calls on the detached store; their Ok replies report lost writes (CONFIRMED)
`runtime.rs:1586-1589` (documented), `:1665-1671` (the table is replaced; the calls map and tasks are untouched).
What: an async call on handle H that is in flight across a restore finishes on the old, detached
store and replies status 0. H now names the restored store, which never saw the write. The host
reads that as success.
Repro (T10): `slow_add(100)` on H=1; restore the snapshot taken before; advance 20 ms -> reply Ok
body 101; `get` on H -> 1.
Fix: on restore, cancel the calls and streams whose receiver handle was replaced or dropped (status
3 / stream end), or reply status 5 `stale_handle` when they finish. Document which one in SPEC 5.9.

## Low

- **L1. Shutdown is incomplete (CONFIRMED T4, T4b, T9).** `runtime.rs:446-493`, `:1161`, `:1181`,
  `:1374`, `:1507`. In-flight calls and open streams get no terminal reply (T9: call 40 and stream 41
  silent; the Swift host fails its own pendings, Rust and transport clients may hang). `Events`,
  Rust port bindings and extensions are not cleared. `event()` still runs subscribers after
  shutdown (T4). `spawn`, `sleep` and `port_call` are accepted after shutdown (T4b: `tasks:1`). A
  subscriber or task holding a `Ctx` pins the runtime forever (T4: `Weak::upgrade` still `Some`
  after every Arc is dropped). Fix: reply status 3 / stream end in `teardown`, clear
  subs/bindings/extensions, and make those entry points no-ops once `shut_down` is set.
- **L2. InitHooks run before the embedder can bind anything (CONFIRMED T3).** `runtime.rs:385-399`: the
  core thread starts, then hooks run and their tasks are polled 5 us (min) / 20 us (median) / 42 us
  (p99) after `Runtime::new` returns. keel-ffi binds ports after `init` (`session.rs:142-146`) and Swift
  registers adapters later still, so a hook's first port call sees `Unavailable`. keel-query works
  around this with a 50 x 100 ms retry (`shared.rs:916-945`); any other hook or a slower host silently
  loses its start-up work. `run_init_hooks` is public and not idempotent. Fix: a two-phase start
  (`init` then `start()` after ports are bound), or run hooks on the first `call`.
- **L3. Unscoped writes are dropped or misrouted, and counted as delivered (CONFIRMED T11).**
  `runtime.rs:70-75`, `:55-57`. On a non-global runtime (every `Runtime::new`, e.g. transport
  sessions) a write from a thread with no scope is silently dropped. keel-signals thinks it was
  delivered, so it is never re-sent (T11: `count=2` never reaches the host). With a global runtime,
  another runtime's store lands on the global host under a foreign handle. Fix: route by the
  store's owner (the cell knows its handle; give it a runtime id), or log and mark the slots unsent.
- **L4. Write checker covers only pool workers (THEORY).** `runtime.rs:103-105`: any other off-core
  thread passes (the M1 precondition), and `Blocking::Inline` (`blocking.rs:301`) runs the closure on
  the caller's thread. A test that writes signals inside `spawn_blocking` passes on `TestRuntime`
  and panics in a debug native build. Fix: a thread-local "inside a blocking closure" flag set by
  `make_job`.
- **L5. `shutdown()` from the core joins the pool and the timer thread while holding the core lock
  (THEORY).** `runtime.rs:451-462` skips only the core-thread join. A blocking job that takes the
  core (e.g. `rt.snapshot()` for background persistence) waits for the lock, and shutdown waits for
  the job. Fix: when `holds_core()`, signal the pool/timer threads without joining, or defer the
  joins to `Drop`.
- **L6. `Ctx::cancel_task` drops the future off-core (THEORY).** `runtime.rs:1166-1170`: no core lock,
  so user `Drop` code (stores, `PortFuture`, anything captured) runs concurrently with core user
  code, which breaks the one-mutator rule. Fix: defer the drop to the next turn (a cancelled-ids
  queue drained under the lock).
- **L7. Abandoned port ids grow without bound (THEORY).** `ports.rs:244-249`: an id stays until the
  host replies. The host is never told the call was abandoned (SPEC 5.2 says "sends a port cancel
  notification"), so a host that drops such requests leaks one entry per cancel. Fix: cap the set
  or age it out, and add the notification in v1.x.
- **L8. Generation wrap (THEORY, arithmetic).** `object_table.rs:187-192`: after 2^32-1 releases a slot
  returns to generation 1. The LIFO free list concentrates churn on the lowest slot, so 10k
  short-lived handles/s wrap it in about 5 days. Fix: retire a slot at wrap instead of reusing it
  (the H1 high-water mark does this naturally).
- **L9. The R2 fix missed restore Phase 3 (CONFIRMED T12).** `runtime.rs:1684-1700` re-observes with
  no surrounding `keel_signals::txn`. Leftovers from a computed that is still writing at the pass
  cap commit inside `cell.observe`, before the one combined `deliver_entries`. Repro: the R2
  `Chaser` store (computed does `if v < 50 { n.set(v+1) }`), snapshot at n=0, observe, restore.
  `Runtime::observe` converges (host n=50); after the restore the host gets 44 change-sets and
  ends at n=7 while the core is at 50. The same shape lets store B's computed, written during
  Phase 3, overtake store A's already-encoded entries. Fix: wrap Phase 3 (encode and deliver) in
  `keel_signals::txn`, as `54b3d14` did for `observe`.

## Notes

- N1. `dispatch_to_rust` (`runtime.rs:1358-1369`) is unguarded: a panicking Rust fake unwinds through
  `port_call`/`port_call_sync` into the caller (caught by the task guard if there is one; not from
  embedder code).
- N2. `core_loop` (`runtime.rs:1799-1812`) catches a panic in `run_batch`, but the rest of that batch
  is dropped with `queued == true`, so those tasks are never re-queued. Requeue the unpolled ids.
- N3. Duplicate `StoreRestorer` / `PortDispatcher` ids silently go to the first registration
  (`runtime.rs:335-344`), unlike dispatcher collisions, which are logged.
- N4. `stats_json` reports `"transactions"` as the change-set count (`runtime.rs:1737`).
- N5. `observe` records `Observed` before `cell.observe` (`runtime.rs:1086-1101`); if the latter
  panics, a later restore re-observes signals the host never subscribed to.
- N6. Restore Phase 3 matches observations by raw handle and ignores type (`runtime.rs:1688-1698`).
  A snapshot from another session that puts a different store type at H sends that type's entries
  to a mirror typed for the old one.
- N7. `MAX_RESTORE_INDEX = 2^20` (`object_table.rs:44-47`): one crafted record makes the table allocate
  about 1M slots and a 1M-entry free list, which every later `clear` and `raise_min_generation` then
  walks.
- N8. `E_REENTRANT` on void entry points (`release`, `cancel`, `observe`) is a logged no-op: a host
  finalizer that runs inside a callback leaks the handle for good. A deferred queue would be kinder.
- N9. Txn ids (`keel_signals::next_txn_id`) and runtime ids are process-global, so parallel
  `TestRuntime`s see interleaved ids. Keep them out of golden files.
- N10. `RecordingHost::wait_until` uses `Instant::now()` (`testing.rs:320`), which panics on
  wasm32-unknown-unknown if called. Internals section 16 says `Instant` is never used on wasm.

## Is keel-runtime sound for v1?

Mostly. The core machinery holds up: executor, call and cancel gating, stream credit, port
abandonment, timers, panic containment and restore input validation. I found no double reply, lost
wake-up, escaping panic, or deadlock on any path where the host keeps its contract. It is not ready
where restore is used in a live runtime (H1, M3, L9), where signals are written off the core (M1,
L3, L4), or where the host misbehaves inside a callback (M2 hangs instead of raising `E_REENTRANT`).

Fix first:
1. **H1**: a per-slot generation high-water mark, so restore never re-issues a generation the host
   may hold (and a fresh-runtime floor carried in the snapshot, ADR).
2. **M1 (+ L9)**: deliver observe/restore entries under the store's delivery lock, with a txn id
   allocated there, inside a txn, and narrow `write_allowed` so off-core writers are the exception.
3. **Shutdown and restore hygiene (M3 + L1)**: terminal replies for in-flight calls and streams on
   shutdown and on restore of their receiver; clear events, bindings and extensions; refuse
   spawn/sleep/port_call/event after shutdown. Close M2 in the same pass with a thread-local
   "in host callback" flag checked by `enter_core`.
