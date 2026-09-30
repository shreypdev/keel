# ADR-023: observe and restore deliver under the store's lock; host callbacks are re-entry-proof; restore and shutdown answer what they cancel

Status: accepted (2026-09-30). Touches SPEC 3.5, 3.7, 5.1, 5.2, 5.9, 6 and 16.1 (`StoreCell`
delivery, `E_REENTRANT`, in-flight calls across `restore` and `shutdown`, the write checker).
Origin: `.10x/reviews/2026-09-30-keel-runtime-review.md`, findings M1, M2, M3, L1, L3, L4, L5, L6,
L7, L9. ADR-022 covers H1.

## 1. M1 + L9: observe and restore deliver through one path, under the delivery lock

Context. `Runtime::observe` built entries with `StoreCell::observe` (no delivery lock), then took a
`txn_id` and called `Host::change_set` outside the lock. A commit of the same store on another
thread could claim the slot, take a smaller id and deliver the newer value first; the observe's
older entries then arrived last and the host mirror ended on a stale value, against ADR-020 ("one
store's change-sets reach the sink in claim order with increasing ids"). Restore phase 3 had the
same shape and also lacked the `keel_signals::txn` that `54b3d14` gave `observe`: pass-cap leftovers
committed inside `cell.observe`, before the combined delivery (L9: host at 7, core at 50).

Decision.
* `keel-signals` gains `StoreCell::observe_and_deliver(signal_ids: &[u32], deliver: impl FnOnce(&[u8])) -> u32`.
  It takes the store's delivery lock, builds the settled entries (same algorithm as `observe`),
  allocates the `txn_id` under the lock and records it in `last_txn` (so a commit holding an older
  shared id replaces it), and calls `deliver` with the complete change-set while still holding
  the lock. The transaction guard is declared before the lock, so leftover writes commit after the
  lock is released. A panic in encoding or `deliver` rolls the observe back (nothing stays
  observed that the host never received; already-observed targets are marked unsent).
  Sink contract unchanged: `deliver` must not wait for another thread writing the same store.
* `Runtime::observe` and restore phase 3 both call it, inside `keel_signals::txn`. Restore now
  delivers **one change-set per re-observed store** (was one combined change-set): holding several
  stores' delivery locks at once would add a lock-order hazard for no benefit, and commits already
  deliver per store. The host applies them in order, exactly as for commits.
* `Observed` is recorded after a successful observe (review N5).

## 2. M2: a host callback may not re-enter the runtime, whichever lock the thread holds

Context. `E_REENTRANT` detection tracked only the core lock. An off-core commit holds the store's
delivery lock while `Host::change_set` runs on its own thread; a host that called back into the
runtime from there blocked on the core, while the core, inside a dispatch writing that store,
blocked on the delivery lock (ABBA). SPEC 5.1 promises detection; it hung instead.

Decision. The runtime sets a per-thread "inside a host callback" marker (a stack of runtime ids,
so a callback of runtime A may still use runtime B) around **every** `Host` call: `reply`,
`change_set`, `stream_item`, `port_call`, `timer_set`, `log`, `schedule`. `enter_core`, `poll`
and `run_pending` refuse (`E_REENTRANT`, same reporting as before) when the marker for that runtime
is set, in addition to when the thread holds its core lock. `port_reply`, `timer_fired`,
`stream_credit`, `snapshot` (best effort, lock-free when refused) and `stats_json` stay allowed.
`Runtime::shutdown` from a host callback or the core is a contract violation (debug assertion,
L5): it would join the very thread it runs on or wait for a job that waits for the core.

## 3. M3: restore cancels the calls whose receiver it replaced

Context. `restore` replaces the table but left in-flight calls running on the old, detached
object; an async call on handle H that finished after the restore replied status 0 for a write the
restored store at H never saw.

Decision. After the table is rebuilt, the runtime cancels every in-flight call and stream whose
receiver handle was replaced or invalidated (the old object is not the object now at that handle;
after a restore that is every call with a non-null receiver). A plain call gets **status 3
(cancelled) exactly once**; a stream is ended with a **`StreamItem` flag 2 (error) whose body is a
`String`** (`"cancelled: ..."`, the same shape as a stream panic), because the host did not ask for
the cancel and flag 1 would read as a clean end; a silent close (what `keel_cancel` does) would
strand the host. The tasks are dropped. Calls with no receiver (free functions, constructors) are
unaffected.

## 4. L1 + L5: shutdown answers and releases everything

Decision. `shutdown` first drains the call table: every in-flight call gets status 3, every open
stream the same flag-2 `"cancelled: the runtime shut down"` item, each exactly once (the call
table is the gate, as for cancel). Then the threads stop, pending port calls fail with `Cancelled`,
event subscribers and Rust port bindings are cleared (they may hold a `Ctx`, a reference cycle
that kept the runtime alive), and tasks and objects drop. After `shutdown`, `spawn`, `sleep`,
`port_call` and `event` on a surviving `Ctx` are no-ops that log a WARN (never a panic, never
queued): `spawn` drops its future and returns an inert id, `sleep` completes at once, `port_call`
resolves to `PortError::Cancelled`. Extensions are not cleared: `Runtime::extension` returns `&T`
for the life of the runtime, so a value cannot be dropped while a reference may exist; an extension
that holds a `Ctx` must release it itself.

## 5. L3 + L4: the write checker is an allowlist

Context. ADR-020 chose a blocking-pool *denylist* and rejected a strict checker because tests wrote
store signals from the test thread. It let every other off-core thread write (the M1 precondition),
left writes from a thread with no runtime scope silently dropped (L3), and `TestRuntime` ran
blocking closures inline, so a test that wrote signals in `spawn_blocking` passed where a native
debug build panics (L4).

Decision. In debug builds a write with consequences is allowed only on a thread that holds a
runtime's core lock (a dispatch, a task poll, `observe`, `restore`, an event subscriber, anything
entered through the runtime's entry points) or on a `TestRuntime` driver thread (the thread that
created it). Everything else trips the checker with the documented panic; release builds do not
evaluate it, so the lock-level guarantees of section 1 are what protects a release embedder.
`TestRuntime` runs `spawn_blocking` closures on a real pool thread, so the rule is exercised. A
`testing::unchecked_writes` helper lifts the checker on one thread for tests that must prove the
release-build behavior. This supersedes alternative (c) of ADR-020.

## 6. L6 + L7: smaller hardening

* `Runtime::cancel_task` drops the cancelled future with the core lock held (and `Ctx::cancel_task`
  from inside a poll defers as before), so user `Drop` code never runs concurrently with core user
  code.
* The abandoned-port-call set is capped at 4096 with FIFO eviction and a WARN per eviction; an entry
  is still removed when its reply arrives. A reply for an evicted id is logged as unknown.

## Consequences

* A host sees `status 3` replies and flag-2 stream items where it previously saw silence or a
  wrong `Ok`; all three platform runtimes already treat status 3 and flag 2 as terminal.
* A restore is N change-sets (one per re-observed store), each self-consistent.
* `StoreCell::observe_and_deliver` is new `keel-signals` surface (SPEC 16.1).
* Hosts that called `release`/`observe` from inside an off-core callback now get the documented
  `E_REENTRANT` no-op instead of working by luck (and deadlocking by bad luck); N8 (a deferred
  queue for void entry points) remains open.
* Residual, not fixed: a release-build embedder that writes signals off-core is unchecked (the
  lock-level ordering still holds); `Runtime::init` still runs `InitHook`s before the embedder can
  bind ports (L2, documented in SPEC 6).
