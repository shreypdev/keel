# ADR-034: anything that outlives a call holds a `WeakCtx`, and a runtime ends when its owner lets go

Status: **Accepted** (2026-10-01; implemented on `wt/runtime-lifecycle`, Track A, piece A1. Proposed the same day
from the v1.x gap audit `.10x/specs/2026-10-01-v1x-gaps.md`, gaps LC-1…LC-3 and PA-6). Touches SPEC 5.1 (Shutdown), 5.3 and 16.2 (`Ctx`, the new `WeakCtx`, `Events`),
`undra-runtime` (`ctx.rs`, `runtime.rs`, `ports.rs`), `undra-query` (its tasks and subscribers), `undra-ports`
(the event helpers), `undra-macros` (a `WeakCtx` store field restores like a `Ctx`), and the Kotlin runtime's
`close()` with one JNI native added (`UndraNative.shutdown()`, SPEC 6.1). **No wire change, no C ABI or wasm
ABI change, no schema change, no generated platform code change.** Constitution R11: the runtime model (who keeps a runtime alive, how its work ends) changes, so it is
decided here before code.

## Context

`Ctx` is a clone of `Arc<Runtime>` (`crates/undra-runtime/src/ctx.rs:66-67`). A `Ctx` handed to a call is fine:
it lives as long as the call. A `Ctx` kept by something the runtime itself owns is a reference cycle:

* **The runtime's own tasks.** Every async call task and every stream task captures `self.me()`, a strong
  `Arc<Runtime>` (`runtime.rs:1271`, `runtime.rs:1317-1319`, `drive_stream` at `:2357`). The executor owns the
  task; the task owns the runtime.
* **App tasks, subscribers and stores.** `ctx.spawn(async move { loop { ctx.sleep(d).await; .. } })`, an event
  subscriber closure that captures a `Ctx`, and a store with a `Ctx` field (the pattern E0001's help text for a
  `Ctx` parameter recommends: "store a `Ctx` in the object at construction") all close a cycle through the
  executor, `Events` or the object table.
* **`undra-query`.** The hydration hook (`crates/undra-query/src/lib.rs:130-133`), fetch, persist, replay and GC
  tasks hold strong `Ctx`s; the GC task sleeps `gc_ms` (5 minutes by default) before it runs
  (`crates/undra-query/src/shared.rs:797-800`).

ADR-023 made `shutdown` break every cycle (it drops tasks, subscribers, Rust bindings and objects), so an
embedder that calls it is fine. The audit's probe (threaded `Runtime::new`, the owner's `Arc` dropped without
`shutdown`) shows what happens otherwise: a runtime with a looping task or a subscriber that captured its `Ctx`
is still alive after the owner let go — with its `undra-core`, timer and blocking threads running — and stays
alive; even an idle runtime survives 6 to 8 seconds (the hydration retries) before it is freed. `Runtime::drop`
(`runtime.rs:2314-2330`) already tears everything down; it just never runs.

After `shutdown`, a `Ctx` held outside the runtime (a user thread, a blocking closure, a host object) still
works: `spawn`, `sleep`, `port_call` and `event` are WARN-logged no-ops and `sleep` completes at once (SPEC 5.1).
The holder has no typed way to learn the runtime is gone, and a polling loop outside a task spins.

On the platforms, Swift's `shutdown()` calls `undra_shutdown`; Kotlin's `close()` on an in-process core detaches
the host and leaves the native core running — its tasks, timers and port traffic continue — and refuses a later
`load` in the same process (`UndraCore.kt:221-226`, `InprocTransport.kt:93-99`, `:174-176`); TS drops the wasm
instance (the wasm ABI has no shutdown export, and dropping the instance ends everything).

## Decision

1. **`WeakCtx`.** `undra-runtime` gains `pub struct WeakCtx` (`Clone + Send + Sync + Debug + 'static`, a
   `Weak<Runtime>` plus the runtime id) and `pub enum Gone { ShutDown, Dropped }` (`Display`, `Error`, `Copy`).
   * `Ctx::downgrade(&self) -> WeakCtx`;
   * `WeakCtx::upgrade(&self) -> Result<Ctx, Gone>`: `Err(ShutDown)` once `shutdown` has begun even while the
     memory is still alive (a holder never revives a shut-down runtime), `Err(Dropped)` after the last strong
     reference went;
   * `WeakCtx::is_alive(&self) -> bool` (cheap, for loop conditions);
   * re-exported from the facade's prelude next to `Ctx`.
2. **`Ctx::closed()`.** A future that completes when the runtime starts shutting down (or immediately if it
   has); `WeakCtx::closed()` likewise, completing at once when the runtime is gone. Long-lived work selects on it
   instead of polling. Implemented with the executor's `Notify`, signalled by `shutdown` and by `Drop`.
3. **The rule.** *A `Ctx` lives for a call or a task step; anything that outlives the call keeps a `WeakCtx`.*
   `Ctx` itself stays a strong handle (call-scoped use, `Ctx::current()`, `ctx.runtime()` returning `&Runtime`
   and `extension()` returning `&T` all keep working unchanged and at today's cost).
4. **The runtime's own tasks hold `Weak<Runtime>`.** `spawn_call` and `open_stream` capture a weak reference;
   `finish_call` and `drive_stream` upgrade at the moment they reply or emit and do nothing when the runtime is
   gone (shutdown has already answered every call and stream exactly once, ADR-023). A stream upgrades per item
   and between waits for credit, never across an `.await`.
5. **First-party layers follow the rule.** `undra-query`'s hydration, fetch, persist, queue-replay and GC tasks
   hold a `WeakCtx` and upgrade around each step; a GC timer no longer pins a runtime for five minutes. The
   `undra-ports` event helpers and `undra-query`'s connectivity/lifecycle subscribers stop capturing a `Ctx`
   (decision 6).
6. **Subscribers receive the context.** `Events::subscribe`'s handler becomes
   `Box<dyn Fn(&Ctx, &[u8]) + Send + Sync>`; the generated typed helpers (`on_connectivity_changed(ctx, |ctx,
   online, kind| ..)`, `on_lifecycle_changed`, and the accessors `#[undra::port(event)]` generates) pass it on. A
   subscriber therefore never needs to capture a `Ctx`. (Rust API change, pre-publication.)
7. **Stores may keep a `WeakCtx`.** `#[undra::store]` treats a `WeakCtx` field like a `Ctx` field for automatic
   restore (downgraded from the restore context); E0001's help for a `Ctx` parameter and the store docs recommend
   `WeakCtx` for a context kept in an object. A `Ctx` field keeps working (its cycle is broken at shutdown, as
   today).
8. **Owners.** The `Arc<Runtime>` returned by `Runtime::new`/`init` is the owner. With 4–7, a runtime whose app
   code keeps only `WeakCtx`s across awaits is freed when the owner drops it, and `Drop` performs the teardown.
   Because a call no longer pins the runtime, `Drop` must also answer what is in flight exactly as `shutdown`
   does (status 3 for calls, the cancelled stream item for streams, each once — the `Host` is still owned by the
   runtime while it drops), so a host never waits for a reply that cannot come. `Drop` may run on the core thread
   (the last upgrade released at the end of a poll); it then skips joining itself, as it does today. The global runtime's owner is the
   global slot; `undra_shutdown` remains the only way to end it. **Kotlin's `UndraCore.close()` on an in-process
   core calls the native shutdown** through a new JNI native, `UndraNative.shutdown()` (the shim exports
   `abiVersion` … `statsJson` today but no shutdown, SPEC 6.1; the new entry runs what `undra_shutdown` runs and
   releases the `Callbacks` global reference), and a later `UndraCore.load` in the same process initialises a new
   core (the C ABI supports init → shutdown → init, the FFI review ran 3,000 C cycles; the JNI path gets the same
   test). Swift's `shutdown()` and TS's `close()` already end the core; the three
   runtimes document one meaning: *closing ends the core's work, in-flight calls fail with a typed error, and a
   new load starts fresh*.
9. **Typed end for long-lived loops.** `WeakCtx::sleep(d) -> impl Future<Output = Result<(), Gone>>` resolves
   `Ok(())` after `d`, or `Err(Gone)` as soon as the runtime starts shutting down or is dropped (it races the
   timer against `closed()`), and holds only the weak reference while pending. The periodic-task idiom becomes
   `while weak.sleep(d).await.is_ok() { let Ok(ctx) = weak.upgrade() else { break }; work(&ctx).await }`, which
   ends with a typed outcome instead of spinning. The SPEC 5.1 behaviour of a surviving strong `Ctx` is unchanged
   (`spawn`/`sleep`/`port_call`/`event` stay WARN-logged no-ops and `Ctx::sleep` still completes at once, so no
   caller can hang); after shutdown `WeakCtx::upgrade` is `Err(Gone::ShutDown)`.

## Alternatives considered

* **Make `Ctx` weak.** Breaks every cycle by construction. Rejected: `Ctx::runtime() -> &Runtime` and
  `Runtime::extension::<T>() -> &T` (used by `undra-query` and `undra-ports` on every call) cannot hand out a
  reference through a `Weak`; every call path would pay an upgrade (two atomics and a branch); and a `Ctx`
  obtained in a call could die mid-call, which is a worse failure than a leak.
* **Auto-shutdown when the last *external* reference goes** (count owner handles separately from `Ctx` clones).
  Rejected: the count is unreliable (any internal `Ctx` clone can escape to user code), and shutdown from a
  `Drop` that may run on the core thread cannot join the threads it stops (ADR-023 L5).
* **Leave it to `shutdown`.** It is correct for embedders that call it, and ADR-023 made it so. It leaves
  `Runtime::new` embedders, tests and Kotlin's `close()` leaking threads, and gives long-lived holders no typed
  end — the founder's "a dropped or shut-down runtime is released and the task ends with a typed outcome".
* **A lint that refuses `Ctx` fields in stores.** Too strict: `Ctx` fields are correct when the embedder shuts
  down, and the playground uses them. A recommendation plus decision 7 is enough.

## Consequences

* A runtime dropped by its owner is freed (threads joined) once app code keeps no strong `Ctx` across awaits;
  the audit's probes become regression tests.
* `Events::subscribe` and the generated event helpers change signature (Rust only; every in-repo caller is
  updated in the same piece). Event helpers in user crates are regenerated by the macro.
* Kotlin: `close()` ends the native core; a test or an app that relied on the core running after `close()`
  breaks loudly (calls fail `closed`). A second `load` works.
* No change on the wire, the C or wasm ABI, the schema or the generated platform code; the JNI shim gains one
  native (`shutdown`); the hot sync-call path is untouched (decision 3).
* Residual: app code that keeps a strong `Ctx` in a long-lived place still pins the runtime until shutdown.
  The cookbook says why and shows the `WeakCtx` pattern; `stats_json` gains `strong_refs`
  (`Arc::strong_count` minus the runtime's own) so a leak is visible in devtools (B4).

## Implementation brief

1. `crates/undra-runtime/src/ctx.rs`: `WeakCtx`, `Gone`, `Ctx::downgrade`, `WeakCtx::{upgrade, is_alive,
   closed}`, `Ctx::closed`; rustdoc with the rule of decision 3 and an example of a periodic task that ends on
   `Gone`. Re-export from `lib.rs` and the facade prelude (`crates/undra/src/lib.rs`).
2. `crates/undra-runtime/src/runtime.rs`: a `closed: Notify` (or a list of wakers) on `Runtime`, signalled at
   the top of `shutdown` and in `Drop`; `spawn_call`/`open_stream`/`drive_stream` take `Weak<Runtime>`; audit
   every other `self.me()` captured by a task (`grep -n "me()"`); `WeakCtx::sleep` races the timer
   (`timer.rs`) against the closed notification.
3. `crates/undra-runtime/src/ports.rs`: `EventHandler = Box<dyn Fn(&Ctx, &[u8]) + Send + Sync>`; `deliver_event`
   passes the current `Ctx`. `crates/undra-macros/src/impl_/port.rs` (event accessors) and
   `crates/undra-ports/src` (`on_connectivity_changed`, `on_lifecycle_changed`, fakes) follow.
4. `crates/undra-query/src`: `lib.rs` (hydration hook), `shared.rs` (fetch `:605`, GC `:797`, persist `:823`,
   `start` `:765`), `queue.rs` (`replay_queue` `:261`) hold `WeakCtx` and upgrade per step.
5. `crates/undra-macros/src/impl_/store.rs`: a `WeakCtx` field is restorable state (`ctx.downgrade()`), next to
   the existing `Ctx` rule (E0013's text mentions it).
6. `crates/undra-ffi/src/jni_shim.rs`: register `shutdown` (`()V`) in `JNI_OnLoad`'s `RegisterNatives` table,
   guarded like every entry, dropping the `Callbacks` `GlobalRef` after the runtime stopped; `UndraNative.kt`
   declares it. Kotlin `InprocTransport.kt`: `close()` calls `native.shutdown()` (outside any callback; from a
   callback it is refused like `snapshot`), the `claimed` set releases the native library so `UndraCore.load` can
   init again; `UndraCore.close` docs and SPEC 6.1 updated.
7. SPEC 5.1, 5.3, 16.2: `WeakCtx` (`upgrade`, `is_alive`, `closed`, `sleep`), `Ctx::closed`, the rule of
   decision 3, the subscriber signature.
8. Tests (each asserts `Weak<Runtime>::upgrade().is_none()` within 100 ms of the owner's drop and that the
   `undra-core`/timer/pool threads exited): an open infinite stream (the host receives the cancelled item once);
   an in-flight async call blocked on a port (the host receives status 3 once); a
   spawned periodic task holding only a `WeakCtx`; a released query handle with GC pending; a subscriber using the
   passed `Ctx`; a store with a `WeakCtx` field, snapshot → restore. Plus: `upgrade` is `Err(ShutDown)` after
   shutdown and `Err(Dropped)` after drop; `closed()` completes on shutdown; `WeakCtx::sleep` resolves
   `Err(Gone::ShutDown)` when shutdown starts mid-sleep and does not keep the runtime alive; Kotlin `close()` then `load()` in
   one JVM works and a periodic core task makes no port call after `close()`.
9. Contract scenario (all three runtimes): "close ends the core's work" — after close/shutdown, no port call
   reaches the host and a new load starts with fresh handles.
10. Bench: `streams/*` rows (one upgrade per item) within budget; sync-call rows unchanged.

## Dependencies

Builds on ADR-023 (shutdown answers and releases everything) and ADR-022 (generations survive re-init). Does
not depend on ADR-035/036/037; lands first in `wt/runtime-lifecycle` together with them.

## Amendment A (2026-10-01) — what a call pins

Decision 8 says "because a call no longer pins the runtime". That is precise only for **the task**. The runtime's
own call and stream tasks hold a `WeakCtx` and upgrade per item (decision 4), so an idle stream between items and a
call that has finished hold nothing, and a runtime whose app code keeps only `WeakCtx`s is freed when its owner lets
go. It is not true of the method's **future**. A call whose future owns a `Ctx` keeps the runtime alive until that
future completes or `shutdown` runs: an `async fn f(ctx: Ctx)` body awaiting a port, a generated port proxy
(`ctx.kv()`, `HttpProxy(Ctx)`, each of which holds one), and `undra-query`'s `Kv` calls (hydration, persist, fetch hold a
strong `Ctx` for the length of one port await, "a step" in the sense of decision 3). The adversarial review found it
(M2): LC-1's regression test passed only because its fixture calls `rt.port_call` and holds no `Ctx`, which is not the
shape the macros generate.

The pin is bounded, and explicitly so. It lasts until the future completes (the port answers; an `Http` request
carries its own `timeout_ms`) or until `shutdown` runs, whichever comes first. `shutdown` answers the call (status 3)
and drops its future, which releases the `Ctx`. When the future completes after the owner has already let go, the
`Ctx` it releases is the last strong reference, so `Drop` runs the same teardown (decision 8) and the runtime ends
there. So dropping the last owner does end a runtime that has no call in flight, and ends one that has once its
calls finish or the embedder calls `shutdown`; nothing is pinned by a call that finishes. What stays unbounded is the
single combination of a port that never answers and an owner that never calls `shutdown` (the dev server calls it when a session ends), and `stats_json`'s `strong_refs` shows it (the residual bullet above).

**Accepted for v1.1.** `Ctx` by value is the ergonomic signature of an async method, and the bound above is explicit.
SPEC 5.1 ("Owners and `Drop`") states the residual in the same words, and
`residual_an_async_call_that_keeps_its_ctx_across_a_port_await_pins_the_runtime_until_shutdown`
(`crates/undra-runtime/tests/weak_ctx.rs`) asserts today's behaviour: the call pins a dropped runtime, nothing answers
it, and `shutdown` answers it once and releases everything. The test is meant to **flip** if the residual is ever
removed. The candidate is a port proxy that holds a `WeakCtx` and upgrades around the call, so a dropped runtime ends
the await with a typed `PortError` instead of waiting on the port; that changes what `ctx.kv()` and its siblings are
and would need its own ADR (R11) and a generated-shape review, and is not decided here.

Decision 8's phrase is read as: *the runtime's own call task no longer pins the runtime*. Consequences above stand
otherwise; the residual bullet is extended by the paragraph above.
