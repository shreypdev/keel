# keel-runtime internals: the threading model as built

This document describes what `keel-runtime` actually does, so that the next person to touch
the locking or the cancellation paths does not have to reverse-engineer it. It is the
as-built companion of `docs/SPEC.md` sections 5.1, 5.2 and 5.6 to 5.9. Where the code and the
spec differ, the difference is listed in [Deviations](#deviations-from-the-spec).

Contents

1. [Threads](#1-threads)
2. [The core lock](#2-the-core-lock)
3. [Every lock, in order](#3-every-lock-in-order)
4. [Re-entrancy: `E_REENTRANT`](#4-re-entrancy-e_reentrant)
5. [The executor](#5-the-executor)
6. [Life of a call](#6-life-of-a-call)
7. [Cancellation](#7-cancellation)
8. [Streams and credit](#8-streams-and-credit)
9. [Ports and events](#9-ports-and-events)
10. [Timers](#10-timers)
11. [The blocking pool](#11-the-blocking-pool)
12. [Change-sets, the sink and ordering](#12-change-sets-the-sink-and-ordering)
13. [Objects, snapshot and restore](#13-objects-snapshot-and-restore)
14. [Panics](#14-panics)
15. [Lifetime, shutdown and reference cycles](#15-lifetime-shutdown-and-reference-cycles)
16. [wasm and the test runtime](#16-wasm-and-the-test-runtime)
17. [Deviations from the spec](#deviations-from-the-spec)
18. [How it is tested](#18-how-it-is-tested)

## 1. Threads

| Thread | Started | Does | Holds the core lock? |
|---|---|---|---|
| **caller threads** (host UI thread, JNI threads, Swift tasks, test threads) | by the host | run `call_sync`, `call`, `cancel`, `observe`, `release`, `event`, `snapshot`, `restore` | yes, for the duration of the call, except `port_reply`, `timer_fired`, `stream_credit` and `stats_json`, which never take it |
| **`keel-core`** | `Runtime::init` / `Runtime::new`, unless `core_threads == 0` or wasm | the executor loop: wait for ready tasks, lock, poll up to 64, unlock fairly, repeat | yes, while polling a batch |
| **`keel-timer`** | first `sleep` that needs the internal timer (native, not test runtimes) | sleeps until the earliest deadline in a `BinaryHeap`, completes sleepers | never |
| **`keel-blocking-N`** (up to `min(4, cores)`, or `blocking_threads`; test runtimes too) | on demand by `spawn_blocking` | run blocking closures | never |

There is **one mutator at a time**: whoever holds the core lock *is* the core loop. That holds
whether the holder is the `keel-core` thread polling a task or a caller's thread running a
synchronous method. This is what lets a `#[keel::api]` method mutate its store without any
lock of its own.

Configuration: `core_threads == 0` starts no `keel-core` thread; the host drives the
executor by calling `Runtime::poll` (this is always the case on wasm, and is what `Runtime::new`
gives a native embedder that wants to own the loop).

## 2. The core lock

`Runtime.core` is a `parking_lot::Mutex<CoreState>`. `CoreState` is deliberately tiny (a turn
counter). The lock exists to **serialise the execution of user code**: dispatchers, task
polls, event subscribers, `restore` functions and destructors of objects and tasks. It does
*not* protect the runtime's own bookkeeping (the task slab, the call table, the port table,
the timer heap, the object table), because that bookkeeping has to stay reachable from places
that must not wait for the core lock:

* a `Waker` fired by a blocking thread, the timer thread or a host thread;
* `port_reply` and `timer_fired`, which a host may call from inside `Host::port_call` /
  `Host::timer_set` (wasm) or from its own threads while the core is busy;
* `stream_credit`, which a host calls as soon as it sees status 4;
* code that already runs under the core lock and needs the object table (a dispatcher
  resolving its receiver): `parking_lot` mutexes are not re-entrant, so the table cannot
  live *inside* `CoreState`.

Everything that runs user code, drops user data, or invokes a host callback holds the core
lock. That is why `release`, `cancel`, `observe`, `event`, `snapshot` and `restore` take it
too: they can drop objects or futures, or emit change-sets, and those must happen "on the core".

`CoreGuard` (returned by `Runtime::enter_core`) also
* records the runtime's id in a thread-local (`HELD`), which is how re-entrancy is detected;
* sets `Ctx::current()` for the duration (`CtxScope`), which is how signal writes find their
  runtime (section 12);
* unlocks with `MutexGuard::unlock_fair` at the end of an executor turn so a host call that is
  waiting gets the lock instead of the loop re-acquiring it (tested: a task that re-wakes
  itself forever does not starve `call_sync`).

## 3. Every lock, in order

Acquisition order, outermost first. A lock further down is never held while acquiring one
above it.

| # | Lock | Type | Protects | Held while calling out? |
|---|---|---|---|---|
| 1 | `Runtime.core` | `Mutex<CoreState>` | mutual exclusion of user code | yes: user code and host callbacks run under it, by design |
| 2 | `GLOBAL` | `Mutex<Option<Arc<Runtime>>>` | the process-global runtime | never |
| 3 | `ObjectTable.inner` | `RwLock` | slots, generations, free list, store index, observed sets, poison flags | never (objects are cloned out and dropped outside) |
| 4 | `Runtime.calls` | `Mutex<HashMap<u32, CallEntry>>` | in-flight calls: task id and stream state per `call_id` | never |
| 5 | `Shared.tasks` | `Mutex<Slab<TaskEntry>>` | the task slab | never (futures are taken out to be polled, and dropped after unlocking) |
| 6 | `PortTable.pending`, `PortTable.bindings` | `Mutex`, `RwLock` | in-flight port calls, abandoned ids, Rust bindings | never |
| 7 | `Events.subs` | `Mutex` | subscribers | never (callbacks are cloned out first) |
| 8 | `Timers.state` | `Mutex` | sleepers, deadline heap | never (`MutexGuard::unlocked` around `fire`) |
| 9 | `Shared.queue` | `Mutex` + `Condvar` | the ready queue | never |
| 10 | slot locks: `PortSlot`, `SleepSlot`, `Notify`, blocking `Slot` | `Mutex` | one rendezvous each | never (the waker is taken out and woken after unlocking) |
| - | `Blocking.queue`, `Blocking.workers`, `Timers.thread`, `Runtime.core_thread` | `Mutex` | pool queue, join handles | never |

Rules that follow, each enforced by review and by the stress tests:

1. **No callout under a leaf lock.** Never invoke a host callback, a user closure, a `Drop`
   of user data or `Waker::wake` while holding locks 3 to 10. Wakers are always taken out of
   their slot first (`let waker = { lock; slot.waker.take() }; if let Some(w) = waker { w.wake() }`).
2. **Waking never takes the core lock.** `TaskWaker::wake_by_ref` takes only `Shared.queue`
   (and, in inline mode, calls `Host::schedule`, deduplicated).
3. **Host callbacks run under the core lock** when they arise from a dispatch or a poll
   (SPEC 5.1). They are always wrapped in the panic guard (`guard_host`), so a panicking
   host is logged and ignored instead of unwinding into the runtime.
4. **Nothing waits for the core lock while holding another lock** except `shutdown`, which
   joins threads *before* taking it.

## 4. Re-entrancy: `E_REENTRANT`

SPEC 5.1 forbids the host from calling back into the core from a callback. A plain mutex
would deadlock; instead `enter_core` fails with `Reentrant` when either thread-local says the
thread may not enter: `HELD` (this runtime's core lock is held by this thread) or `IN_HOST` (this
thread is inside a `Host` callback of this runtime). Every host callback (`reply`, `change_set`,
`stream_item`, `port_call`, `timer_set`, `log`, `schedule`) runs under a `HostCall` marker, set
by `guard_host` and the executor's `schedule_host`. The second condition covers a callback that
runs on a thread that does *not* hold the core lock, which `HELD` alone missed: an off-core
commit delivers its change-set under the store's delivery lock (lock graph: core to delivery
lock, because a dispatch that writes the store takes the delivery lock while it holds the core).
A host thread that waited for the core from inside that callback would be the other half of an
ABBA deadlock; it now gets the error instead (ADR-023, review finding M2). The marker is per
runtime id, so a callback of runtime A may use runtime B. What each entry point does then:

| Entry point | Result |
|---|---|
| `call_sync` | reply with status 5 and reason `E_REENTRANT: ...` |
| `call` | returns 5, no reply |
| `cancel`, `observe`, `release`, `event`, `poll`, `run_pending` | logged at level 4 (`E_REENTRANT`), otherwise a no-op |
| `restore` | `Err(RestoreError::Reentrant)` |
| `snapshot`, `stats_json` | allowed: read-only, and the thread already holds the lock |
| `port_reply`, `timer_fired`, `stream_credit` | allowed: they never take the core lock |

`snapshot` from inside a callback on a thread that does not hold the core lock runs without the
lock (best effort: the view may be torn across stores if the core is mutating); it never waits.

This check is always on (the spec asks for it in debug builds; it costs a thread-local read).
It only detects re-entry on the *same thread*. A host that blocks its callback thread on
another thread which calls the runtime will still deadlock; the host contract (enqueue,
don't block) is documented on `Host`.

## 5. The executor

* **Tasks**: `Pin<Box<dyn Future<Output = ()> + Send>>` in a `slab::Slab<TaskEntry>`.
  `TaskId = (slab key, serial)`; the serial makes a stale id (a finished task whose slot was
  reused) harmless to `cancel`.
* **Wakers**: `Arc<TaskWaker>` implementing `std::task::Wake`. Each has a `queued: AtomicBool`;
  `wake` does `if !queued.swap(true) { push id onto the ready queue }`, so a burst of wakes
  is one queue entry. `begin_poll` clears `queued` *before* the poll, so a wake that arrives
  during the poll queues the task again and is not lost.
* **Ready queue**: `Mutex<VecDeque<TaskId>>` + `Condvar`. Native: `wake` notifies the condvar.
  Inline mode (wasm, `core_threads == 0`, test runtimes): `wake` sets `scheduled` under the
  queue lock and calls `Host::schedule` once until the next `take_ready`; there is no lost
  wake-up because `scheduled` is set and cleared under the same lock as the queue.
* **Turn** (`Runtime::run_batch`): take at most 64 ids (`BATCH`), lock the core, poll each, then
  `unlock_fair`. `poll()` runs one turn and, if tasks remain, asks the host to call again;
  `run_pending()` loops until the queue is empty.
* **Polling one task** (`poll_one`): `begin_poll` takes the future out of the slab and marks
  the slot `Polling`; the poll runs under the panic guard without holding any slab lock (so the
  task can spawn, cancel or wake others); `end_poll` puts the future back, or removes the
  task if it finished, panicked or was cancelled meanwhile. Begin and end always happen under
  the core lock, so a task is never polled by two threads and a wake cannot observe `Polling`
  from another poller.
* **`Notify`**: single-waiter, permit-based. `notify_one` *always* stores a permit and wakes
  the waiter; the waiter consumes the permit on its next poll. (Storing the permit only when
  nobody waits loses the wake-up: the woken task would see no permit and sleep again. The unit
  test `notify_stores_a_permit_and_wakes_a_waiter` guards this.)

## 6. Life of a call

`Runtime::call` and `Runtime::call_sync` share `dispatch`:

1. Decode the `Call` payload. Undecodable: `call_sync` replies status 5 with `call_id = 0`,
   `call` returns 5 without a reply (there is no id to answer).
2. Take the core lock (or refuse: section 4).
3. Route: `Function` looks the `FunctionMeta` up by `method_id`; `Method` resolves the handle
   to an object, takes its `type_id` and looks the `ObjectMeta` up; `Constructor` looks the
   `ObjectMeta` up by `type_id`; `LazyPage` is answered by the runtime itself from a
   `LazyList` (no dispatcher). The tables are built once from `keel_meta::registrations()`.
   A lookup that misses (unknown function id, unknown constructor type, or a method on an
   object whose type has no `ObjectMeta`) falls through to the `DispatchLayer`s, in
   registration order, until one answers something other than `Unknown`; none answering
   reports the original miss.
4. For `call_sync`, an async-shaped method (by its metadata: `is_async`, `Stream`, `Result<Stream, _>`)
   is refused with status 5 **before** the dispatcher runs, so nothing half-executes.
5. Run the dispatcher under the panic guard with `&Runtime` erased as `&dyn Any`, then unwrap
   the `DispatchOutcome` into a `DispatchResult`.

Outcomes:

| Result | `call_sync` | `call` |
|---|---|---|
| `Sync(Ok/Err)` | reply status 0 / 1, returned | same reply through `Host::reply`, before `call` returns |
| `Async(fut)` | status 5 (`this method is asynchronous`) | spawn a task; `calls[call_id] = task`; the task replies when it finishes |
| `Stream(s)` | status 5 | register the stream, send status 4, spawn the driver (section 8) |
| `Unknown` | status 5 `unknown method 0x...` | same |
| `BadRequest(why)` | status 5 with `why` | same |
| panic | status 2: message, backtrace; level 5 log; store poisoned | same, through the host |

`call_id` rules for `call`: `0` is reserved and refused; an id that is still in flight is
refused (returns 5, and the original call is untouched); the check and the insertion happen
under the core lock so racing duplicates cannot both win (tested with 8 racing threads).
`call_sync` does not register ids.

## 7. Cancellation

`Runtime::cancel(call_id)` takes the core lock, then:

1. **removes the call from `calls`** (exactly-once gate: whoever removes the entry owns the reply);
2. cancels the task: an idle task is removed from the slab and its future is dropped right
   there under the panic guard (this drops captured `Arc`s, `PortFuture`s, `Sleep`s); a task
   that is being polled *by this same thread* is flagged and dropped when its poll returns;
3. for a plain call, replies status 3; for a stream, sends nothing (the host closed it).

The completion path (`finish_call`, run inside the task's own poll) does the same gate in the
other direction: it replies only if it can remove the entry. Because cancel and every poll hold
the core lock, they cannot interleave, so a call gets **exactly one** terminal reply whichever
wins (checked by the randomised stress test over thousands of racing cancels).

Dropping a future cancels what it awaits: a `PortFuture` marks its `port_call_id` *abandoned*
(the host's late reply is recognised and discarded, not logged as unknown); a `Sleep`
deregisters its timer.

`Ctx::cancel_task` does the same for detached tasks.

The runtime also cancels calls itself (`Runtime::abort_call`, same gate: whoever removes the
`calls` entry owns the terminal message): on `restore`, every call whose receiver the restore
replaced or invalidated (ADR-023, finding M3), and on `shutdown`, every call that is still in
flight (finding L1). A plain call is answered with status 3; a stream gets a `StreamItem` with
flag 2 (error) and a `String` body that starts with `"cancelled: "` (the host did not ask for
the end, so flag 1 would read as a clean completion, and all three platform runtimes end the
stream on flag 2). The task is dropped under the panic guard. A call that has no receiver (a free
function or a constructor) is not touched by a restore.

## 8. Streams and credit

`DispatchResult::Stream` becomes one task, `drive_stream`:

```text
loop {
    next = stream.poll_next()                       // polled BEFORE credit is checked
    Some(Ok(item))  -> wait until credit > 0 (Notify), take one, send StreamItem flag 0
    Some(Err(e))    -> remove call, send flag 2 (error), stop      // no credit needed
    None            -> remove call, send flag 1 (end), stop        // no credit needed
}
```

* Initial credit is 0. `Runtime::stream_credit` adds (saturating) and notifies; it never takes
  the core lock, and unknown ids are ignored.
* The stream runs **at most one item ahead** of the host, and terminal markers need no credit,
  so a host that credited exactly N items for an N item stream still hears the end (gRPC
  servers send trailers regardless of the flow-control window for the same reason). No item
  is *delivered* without credit.
* The call is registered in `calls` **before** status 4 is sent. Credit takes no core lock, so a
  host that grants credit the moment it sees the reply must find the stream; the driver cannot
  run before `call` returns, so the reply still precedes the first item.
* A panic in the stream becomes a flag 2 item whose body is a `String` (there is no typed `E`
  for a panic); the store is poisoned.

## 9. Ports and events

* **Foreign is the default.** Any port id without a Rust binding goes through `Host::port_call`.
  A host answers `Sync(PortReply payload)`, `Async` (reply later with `port_reply`) or
  `Unavailable` (also what an unregistered port answers, SPEC 6.3).
* **Rust bindings** (`bind_port`, `bind_dyn_port`) are stored as an `Arc<Arc<dyn Trait>>` behind
  `dyn Any` (a trait object cannot be downcast from `Arc<dyn Any>` any other way) and fetched
  with `Ctx::rust_port::<dyn Trait>(port_id)`, which is what `#[keel::port]`'s accessor
  (`clock(&ctx)`) calls before falling back to the proxy. A binding never reaches the host.
* **Raw calls to a Rust-bound port.** A generated *proxy* always speaks bytes
  (`Runtime::port_call` / `port_call_sync`). If the port id has a Rust binding, the runtime finds
  the `PortDispatcher` that `#[keel::port]` registered through `inventory` (looked up once, at
  `Runtime::new`) and calls `dispatch(imp, method_id, args)`. The answer is `PortDispatch::Sync`
  or `Async`, both `status u8` (0 ok, 1 typed error, 2 unavailable) followed by the body; the
  runtime strips the status into `Ok(body)` / `Err(PortError::Failed(body))` /
  `Err(PortError::Unavailable)`, so a proxy behaves identically over the platform and over a
  fake. A Rust-bound port with no dispatcher is `Unavailable`. `port_call_sync` on a Rust
  binding whose dispatcher answers `Async` is `Unavailable` (a synchronous caller cannot wait).
* **`Runtime::port_call`** registers the call **before** invoking the host, then completes it
  from the outcome. So a reply that arrives on another thread (or from inside
  `Host::port_call`, as a wasm host with a synchronous JS function does) before the host
  returns `Async` is accepted.
* **`port_call_sync`** needs a synchronous answer. `Async` is `Unavailable` *unless the reply
  already arrived* during the host call; otherwise the id is abandoned.
* **Abandoned ids**: `PortTable.abandoned` holds ids whose future was dropped; `begin` never
  hands one out again while it is there; a late reply removes it. Ids wrap, skip `0`, and
  skip anything live or abandoned. The host is never told about an abandonment (v1), so a host
  that drops such requests would leak one id per cancel: the set is capped at `MAX_ABANDONED`
  (4096), oldest first (`abandoned_order`, a FIFO that is compacted when it holds more than
  twice the cap of stale ids), and each eviction logs a WARN through the owning runtime; a late
  reply to a forgotten id is logged as unknown like any other unknown id (ADR-023, L7).
* **`port_reply`** decodes, completes the slot, wakes the task. No core lock.
* **`Events`**: subscribers run in `Runtime::event` under the core lock, in subscription order,
  each under the panic guard. `Subscription::drop` unsubscribes.

## 10. Timers

`sleep(d)` registers a *sleeper* under a fresh `timer_id` **first**, then arms a timer. The
host is asked with `Host::timer_set(id, ceil(d in ms))`; if it owns timers (wasm), the runtime
does nothing more and `timer_fired(id)` completes the sleeper. Otherwise the internal timer
takes it: a `BinaryHeap<Reverse<(deadline_ns, seq, id)>>` served by the `keel-timer` thread
(`Condvar::wait_for` until the earliest deadline; equal deadlines fire in arming order).

* The deadline is measured from the `sleep()` call, not from the first poll.
* Firing takes no core lock: it sets the slot and wakes the waiter.
* A dropped `Sleep` removes its sleeper; its heap entry stays until it expires unless dead
  entries outnumber live ones two to one, when the heap is compacted.
* `d == 0` completes immediately without a timer.

Test runtimes have a **manual clock** and no timer thread. `TestRuntime::advance(d)` first runs
every ready task (at the current time), then repeatedly pops the earliest due deadline, sets the
clock to it, fires it and runs the woken tasks, so a task that sleeps again inside the window
is served in the same call, exactly as with real time.

## 11. The blocking pool

Native: workers are started on demand up to `min(4, cores)` (or `blocking_threads`), named
`keel-blocking-N`, alive until shutdown. `spawn_blocking(f)` queues a job that runs `f` under the
panic guard with the runtime installed as current (`Ctx::current()` works, **without the core
lock**), stores the result, and wakes the awaiting task. A panic in `f` is carried to the
awaiting task and re-raised there with `resume_unwind` (the original message and backtrace
are preserved in a `CarriedPanic`), where the ordinary guard turns it into a status 2 reply.

wasm has no pool: `f` runs **inline, synchronously, inside the `spawn_blocking` call**, and the
returned future is already complete. Test runtimes use the real pool (ADR-023), so a test
exercises the no-writes rule; `TestRuntime::run_pending`, `run_until` and `advance` wait for the
closures to finish and run the tasks they wake, so tests still see results without waiting by
hand.

`f` must not write signals or call host entry points: it does not hold the core lock. Debug
builds enforce the first half: the runtime installs a `keel_signals::set_write_checker` (an
allowlist, see section 12) that refuses signal writes (with consequences: to an attached signal,
or one with dependents) on a pool worker thread, so such a write panics in the closure, and the
panic reaches the awaiting task like any other. Release builds do not check.

## 12. Change-sets, the sink and ordering

`keel-signals` has one process-global `ChangeSink`. The runtime installs a single
`RuntimeSink` (once per process) that routes each committed change-set to
`current_or_global()`: the runtime whose `CtxScope` is active on the committing thread, else
the one created by `Runtime::init`. This is what allows many runtimes (every `TestRuntime`, in
parallel test threads) to share the process.

Consequences:

* Signal writes inside a dispatched call, a task poll, an event subscriber or `Ctx::txn` find
  their runtime automatically. A write from test code outside any of those needs
  `let _scope = ctx.enter();` (or `Ctx::txn`), or it has no runtime to be delivered to (it
  reaches the global runtime if there is one, otherwise it is dropped).
* Delivery happens on the committing thread, under the core lock when that thread holds it.
  Change-sets are therefore delivered in commit order for everything that runs on the core.
  The sink deliberately never takes the core lock: `keel-signals` may call it while holding
  its own lock, and a thread holding that lock that waited for the core while the core
  waited for it would deadlock.
* Debug builds refuse a signal write with consequences on any thread that does not hold a
  runtime's core lock (an **allowlist**, ADR-023): a pool worker, a host or embedder thread and a
  thread inside no runtime are refused; the holders of the core lock, a `TestRuntime` driver
  thread (the thread that created it, or one that called `testing::drive_from_this_thread`) and
  `testing::unchecked_writes` are allowed. Embedders write by spawning onto the runtime
  (`ctx.spawn`) or calling in. In release builds the check does not run and an off-core write is
  delivered from that thread without the core lock. `keel-signals` still
  delivers the change-sets of one store one at a time, in claim order, but a write from another
  thread is not part of the core's transaction (if the slot is already dirty in an open
  transaction it ships with that transaction). Keep signal writes on the core (send the result
  back to a task).
* `observe` delivers the initial change-set synchronously (before `observe` returns) through
  `StoreCell::observe_and_deliver`: the cell builds the entries, allocates the `txn_id` and calls
  the runtime's delivery function, all under the store's delivery lock, inside a
  `keel_signals::txn` (so writes a computed makes that do not settle commit afterwards). A commit
  of the same store on another thread therefore either delivers before it or after it, never
  in between with the observe's older values (ADR-023, finding M1). Restore phase 3 uses the same
  path, once per re-observed store.
* A change-set is delivered before the `Reply` of the call that caused it (the write happens
  inside the dispatch, the reply after).

## 13. Objects, snapshot and restore

**Object table**: a `Vec` of slots, a free list (LIFO) and a `BTreeSet` index of stores (so
snapshots are in handle order). Every inserted object takes a fresh generation from one counter
(`Generations`: a `u32` holding the highest generation issued, `fetch_update`, never wrapping; one
process-wide instance shared by real runtimes, a private one per `TestRuntime` so its handles are
deterministic). A released or cleared slot is just vacant, so its old handles stay stale for good
(`Stale`) instead of aliasing the next object. When all `u32::MAX` generations are spent, `insert`
logs FATAL once and panics with a clear message (ADR-022). A slot holds an `Arc<dyn AnyObject>`; the object outlives its handle while a task
holds the `Arc`. Inserting a store calls `StoreCell::set_handle`; releasing (or a restore
replacing it) calls `set_handle(0)`, so a store that a task still holds stops delivering
change-sets under a handle it no longer owns.

**Which objects are stores.** `insert_object` asks the type's `StoreRestorer::cell` accessor
whether the value holds a `StoreCell` (registered stores do; plain objects do not), so a
generated constructor arm can hand every result to one entry point. Stores get the handle
written into their cell; everything else is a plain object.

**Snapshot** (`Runtime::snapshot`): under the core lock, for each store in handle order, the
runtime asks the cell for `StoreCell::encode_snapshot`, which writes a whole store record
(`handle u64, type_id u32, signal_count u32, signal_count x { signal_id u32, len u32, value }`,
the cell knowing its own handle and type id). The runtime decodes that record and re-encodes it
with the object table's handle and type id, so the snapshot is right even if a cell's handle is
stale (a store restored from an older snapshot, a handle reissued after release). The result is
a valid `keel_wire::payload::Snapshot` (tested by decoding it): `count u32`, then
`generation_floor u32` (the generation counter's high-water mark, read after the stores were
listed, so it is at least every generation in the snapshot), then the records. A cell that panics
or writes a malformed record is skipped and logged. Non-store objects are not included.

**Restore** (`Runtime::restore`), all-or-nothing:

1. Decode and validate the snapshot: no null handle, no generation 0 or `u32::MAX`, no duplicate
   handle, index at most 2^20 (a corrupt snapshot cannot make the table allocate gigabytes), and a
   `generation_floor` below `u32::MAX` (a floor of `u32::MAX` would leave the counter nothing to
   issue: `RestoreError::GenerationFloor`).
2. Build every store through its `StoreRestorer` (`{ type_id, restore, cell }`, one per store
   type, submitted through `inventory` by `#[keel::store]`): `restore(ctx, handle, reader)`
   gets a `Reader` over the **body** only (`signal_count u32`, then `{ signal_id, len, value }`
   per signal; the runtime has already consumed the handle and type id) and the re-issued raw
   handle, tells the new store's cell that handle, and returns it. The runtime checks that the
   reader is fully consumed. Nothing has been touched yet, so any failure
   (`UnknownStoreType`, `Store`, `Panicked`, `Decode`) leaves the runtime unchanged.
3. Replace the table (and cancel the calls whose receiver it replaced, section 7): raise the generation counter to
   `max(current, snapshot.generation_floor, every generation in it)` (never lowering it, so nothing
   issued before the snapshot, between it and the restore, or before a crash can be issued again,
   ADR-022), clear the table (every old handle becomes stale; old stores are detached; the observed
   sets are remembered per handle), and `insert_at` every store at its original index and
   generation.
4. Re-observe, for every restored store whose handle had observations before, exactly those
   signals: inside one `keel_signals::txn`, one `StoreCell::observe_and_deliver` per store, so
   each store gets **one change-set** with its signals' current values, built and handed to the
   host under that store's delivery lock (the same path as `observe`, ADR-023). Writes a computed
   makes that do not settle commit when the transaction ends, after every store's entries, and
   the host converges on the core's values.

Restoring into a fresh runtime (after a crash) has no memory of observations; the host
re-observes what it mirrors.

## 14. Panics

`guard::guarded` wraps every place that runs code the runtime does not own: dispatchers, task
polls, event subscribers, init hooks, `StoreObject::restore`, `Drop` of objects and futures
(`drop_guarded`) and every host callback. A panic becomes a `PanicReport { message, backtrace }`.

A backtrace only exists at the moment of the panic, so the first guard installs (once per
process) a chained panic hook: while a guard is active on the panicking thread it records the
report (message, location, `Backtrace::force_capture()`) in a thread-local instead of
printing; otherwise it defers to the previous hook. The catch site uses the recorded report if
its message matches the payload's, else rebuilds one from the payload. `spawn_blocking` panics
travel as `CarriedPanic`. On wasm (`panic = "abort"`) nothing can be caught; the hook instead
logs the panic at level 5 through the host before the trap (SPEC 7), and the backtrace field is
`"unavailable"`.

What a caught panic does: status 2 reply with `String message, String backtrace` (or a stream
error item), a level 5 log record, `stats.panics += 1`, and the receiver object is marked
**poisoned** (`stats.poisoned_stores`, informational: a poisoned store keeps working, SPEC 5.5).

## 15. Lifetime, shutdown and reference cycles

`Ctx` is an `Arc<Runtime>`. A store that keeps a `Ctx`, an event subscriber or Rust port
binding that captured one, and every in-flight async call, is therefore a **reference cycle**
with the runtime that owns it. `Runtime::shutdown` breaks them, in this order (ADR-023, L1):

1. flag the runtime shut down (later `call`s answer 5) and close the executor (`Executor::shutdown`
   sets `closed` under the `tasks` lock, so a racing `spawn` is refused instead of landing after
   the final `clear`);
2. **under the core lock, before anything slow is joined**: `cancel_all_calls`, which for every
   entry of the call table sends status 3 (a stream: a flag 2 item, `"cancelled: the runtime shut
   down"`) and drops its task (`abort_call`, the same exactly-once gate as cancel). Taking the
   lock here means no poll is running and no `call` is half way through its dispatch: a call
   either registered before (and is answered) or sees the flag and is refused;
3. join the `keel-core`, timer and blocking threads (not when the caller is the core or inside a
   callback; debug builds assert that this is never the case, L5);
4. fail pending port calls with `Cancelled`, clear event subscribers and Rust port bindings
   (`release_user_references`, dropped outside their locks under the panic guard);
5. under the core lock, `teardown`: drain deferred drops, drop every remaining task and object.
   `Runtime::extension` values stay (they are handed out as `&T`; one that holds a `Ctx` must let go
   of it itself).

After `shutdown` a runtime that nothing else references is freed; before it, an idle
runtime with no stores holding a `Ctx` is also freed when the last `Arc` drops (its `Drop`
runs the same teardown without locks, since nothing else can be running). `spawn`, `sleep`,
`port_call` and `event` on a surviving `Ctx` do nothing but log a WARN (the first eight per
runtime; `warn_shut_down`): `spawn` drops the future unpolled and returns `TaskId::dead()`,
`sleep` completes at once, `port_call` resolves to `PortError::Cancelled`, `event` returns.

**Cancelled futures drop on the core (L6).** `cancel_task` drops the cancelled future with the core
lock held: on the spot when the caller already is the core, else under `try_enter_core` if the
lock is free, else it is queued in `deferred_drops` and the core is nudged (a dead task id in the
ready queue), so the drop happens at the start of its next turn. It never waits for the core.

* `Runtime::init` puts a strong reference in the global slot, so a global runtime lives until
  `shutdown` (dropping your own `Arc` does not stop it).
* Concurrent `shutdown` calls are safe; the later callers return immediately while the first
  completes the teardown.
* `Drop` may run on the `keel-core` thread or a blocking worker (if the last reference is held
  by a task or job); it does not try to join the current thread.

## 16. wasm and the test runtime

The wasm shell differs from native only through `cfg(target_family = "wasm")`:

* no `keel-core` thread, no timer thread, no blocking pool (inline);
* `std::time::Instant` is never used; the timer clock is manual and timers are owned by the host
  (`Host::timer_set` returns true), and `sleep` logs a warning if it is not;
* wakers call `Host::schedule` (deduplicated per turn) and the host calls `Runtime::poll`;
* panics cannot be caught (see section 14).

The wasm32 target is not available in this environment. The gated code is compile-checked and
clippy-checked with a fake `--cfg target_family="wasm"` on the host target; CI checks the real
target.

`testing::TestRuntime` is a real `Runtime` built in "manual" mode: inline executor, manual clock,
the real blocking pool (started on demand; the driving calls wait for its closures), a private
generation counter (deterministic handles), no init hooks. The thread that creates it is marked a
test driver, whose direct signal writes the write-context check allows. `RecordingHost` records everything the runtime emits and
answers port calls from a script table. `run_until` drives a future on the test thread and
panics instead of hanging when nothing can make progress.

## Deviations from the spec

| Where | Spec says | Built | Why |
|---|---|---|---|
| 5.1 | `Mutex<Core>` guards the core state | `Mutex<CoreState>` where `CoreState` is a counter; bookkeeping has its own locks | wakers, `port_reply`, `timer_fired` and `stream_credit` must not wait for the core lock, and dispatchers need the object table without re-locking (section 2) |
| 16.2 `DispatchResult` | four variants | five: `BadRequest(String)` added | a dispatcher has no other way to say "the arguments did not decode" (status 5 with a reason) |
| 5.9 / 16.2 | `restore(ctx, values)`; `StoreRestorer { type_id, restore(ctx, reader) }` | `StoreRestorer { type_id, restore(ctx, handle, reader), cell }` over the store body (section 13) | the `keel-macros` branch generates this shape (it needs the handle to attach the cell and a way to find the cell in a `dyn Any`), and the runtime builds on it |
| 5.7 | `bind_port<P>(port_id, imp: Arc<dyn Any>)` | implemented, storing `Arc<Arc<dyn Trait>>`; `bind_dyn_port<P: ?Sized>` and `Ctx::rust_port::<dyn Trait>` are the typed entry points | an `Arc<dyn Any>` cannot be downcast to `Arc<dyn Trait>`, so a sized wrapper is stored |
| new | (none) | `PortDispatcher` / `PortDispatch` (inventory) | a proxy calling a Rust-bound port sends bytes; `#[keel::port]` generates the byte-level entry point (section 9) |
| 5.1 | re-entrancy detected "in debug builds" | detected in every build, for the core lock and for every host callback (`IN_HOST`) | a deadlock in release is worse than a status 5; a callback on an off-core thread could deadlock without the second half (ADR-023) |
| 5.2 | task cancellation drops the future | also drops what it awaits (`PortFuture`, `Sleep`) and replies status 3 exactly once | see section 7 |
| new | (none) | `InitHook`, `Runtime::extension`, `Runtime::new` | `keel-query` needs a hydrate hook and a place to keep the `QueryClient` (SPEC 9) |
| new | (none) | `Notify`, `LazyList`, `StoreRestorer`, `Ctx::enter` | required by the task; see the crate docs |
| 5.6 | dispatch by the static `Registration` tables only | plus `DispatchLayer` (inventory): a call the static table cannot route (unknown function, unknown constructor type, a method on an object whose type has no registered dispatcher) is offered to each layer; a layer answers `Unknown` for ids it does not serve | `keel-query` serves one handle type and one mutation function per user query, ids that exist only as generic instantiations and that must not enter the schema (bindgen synthesizes them from `QueryMeta`); layers see no `sync_only` metadata, so an async answer to `call_sync` is refused after the fact |
| 5.9 | every store is snapshotted | `KeelObjectDyn::transient()`: a store that answers `true` is left out of `snapshot()` | query handles are views of the query cache; without this a snapshot with an open handle could not be restored (`UnknownStoreType`) |

Known limitations, each deliberate for v1:

* `Runtime::run_pending` runs until the ready queue is empty; a task that re-wakes itself
  forever makes it run forever. `poll` is bounded (one turn of at most 64 polls).
* `stream_credit` is not synchronised with a `call` that is still in progress on another thread
  for the same `call_id` (the host must grant credit after it has issued the call).
* Signal writes from off-core threads (release builds only; debug builds refuse them, section 12)
  are not ordered with core writes.
* An `Arc<Runtime>` held only by your code does not stop a global runtime, and a store's `Ctx`
  keeps a runtime alive until `shutdown` (section 15).
* Generations are a process-wide `u32` counter: 2^32 - 1 handles per process, then object creation
  fails loudly (section 13, ADR-022).
* `Runtime::extension` values are not released by `shutdown`; one that holds a `Ctx` pins the
  runtime until it lets go (section 15).
* A host that calls `release` or `observe` from inside a callback gets the logged no-op, not a
  deferred retry (review note N8).
* `InitHook`s run before the embedder can bind ports (review finding L2); the C ABI's
  documentation of port registration (SPEC 6) says how hosts cope.

## 18. How it is tested

* Unit tests sit next to the code: object table, executor (waker dedup, cancel, stale ids,
  deferred cancel), `Notify`, timers (deadline order, compaction, thread), ports (id
  allocation, abandonment), config codec, panic guard, lazy lists, extensions.
* `tests/` uses hand-written stores, dispatchers, restorers and port dispatchers registered
  through `inventory` in the shape `keel-macros` generates (the crates were also checked
  together in a scratch workspace, outside the repo: generated dispatchers, streams, port
  proxies, Rust-bound port fakes and `Ctx::sleep` all run on this runtime): sync/async/stream calls, cancellation, panics, bad requests,
  observe and change-sets, snapshot/restore including every error path, ports and events,
  timers (manual clock and host-owned), and a global-runtime test.
* `tests/threads.rs` runs a real threaded runtime: 8 threads calling `call_sync` on one store
  (running totals must be exactly `1..=N`), a busy core thread that must not starve host
  calls, re-entrant host callbacks refused on both the caller and the core thread, shutdown and
  drop, and a **randomised stress test** (6 threads, seeded, mixing calls, cancels, credit,
  observe, release, snapshot, panics) that asserts every accepted call is answered exactly
  once and that the change-set mirror equals the final state. `KEEL_STRESS_SEED` and
  `KEEL_STRESS_OPS` vary it for soak runs; every test that can deadlock runs under a watchdog.
