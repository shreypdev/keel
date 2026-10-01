# Track A runtime lifecycle (ADR-034, ADR-035, ADR-019 amendment, ADR-036) — adversarial review

**Date:** 2026-10-01 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/runtime-lifecycle` at `480a0f3` (172 files, +7,220/−917 against `main`; `main` merged at `42fdd42`, not re-merged
here) · **Read:** `CLAUDE.md` (R2, R4, R6, R8, R10, R11), ADR-034, ADR-035, ADR-036, the dated ADR-019 amendment,
Amendment C of `.10x/specs/2026-10-01-v1x-default-choice-design.md`, the SDE record
`.10x/decisions/sde/runtime-lifecycle.md`, `docs/SPEC.md` 3.7, 5.1, 6, 16, and the diff (`undra-ffi` `jni_shim.rs`,
`session.rs`, `api.rs`; `undra-runtime` `ctx.rs`, `runtime.rs`, `blocking.rs`, `ports.rs`, `testing.rs`;
`undra-signals` `context.rs`, `signal.rs`, `store.rs`, `computed.rs`; `undra-wire` `payload/mod.rs`; `undra-macros`
`object.rs`, `types.rs`, `check.rs`; `undra-query`; the three runtimes' stream decoders and `InprocTransport`; the
contract runners and S07.6/S07.7/S17.7) · **Method:** a test for every attack (race tests run 40 rounds and 5 release
runs each), Miri on the new and existing FFI shutdown tests, a **shutdown mutant** run through both native contract
runners, release bench budgets and stress gates · **Fixes:** `b3f4646` (a comment in `S17Panic.kt` and a sentence of
the Kotlin NOTES were corrected with this review's commit).

## Verdict

**Merge after fixes; the fixes are on the branch.** The runtime side holds up under attack. The JNI `shutdown` native
is `session::stop`, the same guarded path as `undra_shutdown`, with no `unsafe` of its own; a new test races it against
three host threads inside `undra_call`, `undra_call_sync`, `undra_stream_credit`, `undra_cancel` and
`undra_stats_json`, a second concurrent shutdown and an init from a third thread, and passes natively and under Miri.
"Answered once" holds: the call table is the gate on every terminal path, `Drop` can only run when no thread holds a
strong reference (so never during a poll or a send), and three new race tests (a busy stream against the owner's drop, a
yielding finite stream ending against the drop, a busy stream against two concurrent shutdowns) never see two terminal
items or an item after one. The write check compares runtime ids (a thread-local list of the core locks the thread
holds, ids from a process counter that is never reused), not thread ids, so a reload cannot alias. A panicking computed
leaves the graph consistent (`RestaleOnUnwind` keeps it dirty, the `Recomputing` frame pops on unwind) and a no-op
write behaves as the amendment says. The wire matches ADR-036 and Amendment C (envelope version stays 1). The bench
budget is honest: `signals/set_attached` 8.7 ns against 65 ns, and the change-set path through the new owner registry
is unchanged at 2.19 us.

What was wrong is in the evidence, not the runtime: **S17.7 could not fail** on either native platform for the thing
it exists to catch (M1). A mutant whose `Runtime::shutdown` only released the global slot, leaving the old core thread,
its timer and the `Stress` generator running, passed all 18 scenarios on Kotlin and on Swift. Fixed: with no core
loaded, `undra_stats_json` now reports `runtime_threads`, and both runners require it to be 0 after every close; the
same mutant now fails S17 on both. Second, ADR-034's "a call no longer pins the runtime" is true only for a call whose
future holds no `Ctx`, which is not the shape the macros generate (M2): documented in SPEC 5.1 and pinned by a
characterization test, the ADR wording is the integrator's. No High finding; nothing blocking is open.

## Findings

| # | Sev | Where (at `480a0f3`) | Finding | Status |
|---|---|---|---|---|
| M1 | Medium | `contract-tests/kotlin/src/dev/undra/contract/S17Panic.kt:109-132`; `contract-tests/swift/Tests/ContractTests/S15_S17_Lifecycle.swift:329-358`; `.10x/decisions/sde/runtime-lifecycle.md` ("where a surviving task would show") | S17.7's 200 ms port-call windows cannot see a core that kept running after close. **Kotlin**: `InprocTransport.close()` detaches (`events = null`) before `UndraNative.shutdown()`, so the old core's port calls go to the old, detached callbacks object, and the sleep it had set on the default `Timer` adapter is never reported back to the closed core; a surviving task never reaches the counted adapters, before the reload or after it (the old runtime's `Host` is the old `JniSink`, never the fresh core's callbacks, so the record's "the fresh core's Clock ... where a surviving task would show" is wrong). **Swift**: `shutdown()` detaches the adapters (the `Timer` adapter the generator slept on included) and `session::stop` retires the C-ABI port registrations, so the old core's calls are answered "unavailable". Mutation run: `Runtime::shutdown` reduced to "release the global slot" (the reload still succeeds, the old generator keeps ticking): Kotlin **all 18 pass**, Swift **all 18 pass**. The reload check only catches a `close()` that never calls the native shutdown. | **Fixed.** `undra_stats_json` / JNI `statsJson()` with no runtime reports `runtime_threads` (threads `undra-runtime` started that still run, `testing::live_threads`); Kotlin and Swift S17.7 require `runtime_threads == 0` after the old core's close and after the fresh core's. Same mutant after the fix: Kotlin `S17 FAIL ... timed out after 5000 ms waiting for the old core's threads to exit after close`, Swift `S17 FAIL ... timed out after 5.0 seconds ...`; real code: 18/18 on both. Regression in `crates/undra-ffi/tests/abi.rs` (`shutdown_with_calls_in_flight...` asserts `runtime_threads == 0`; fails at `480a0f3`, which has no such field), and a JNI-level check in `crates/undra-ffi/tests/jni/JniE2E.kt` (close from two threads while four JVM threads call; `runtime_threads == 0`; a new load works). SPEC 6 and 5.1, `scenarios.md` S17.7, both runners' NOTES corrected. |
| M2 | Medium | ADR-034 decision 8 ("Because a call no longer pins the runtime"); `crates/undra-runtime/tests/weak_ctx.rs:386` (`lc1_an_async_call_blocked_on_a_port...`); `crates/undra-query/src/shared.rs:848, 1194` | The runtime's own call task holds the runtime weakly, but the method's future does not: an `async fn f(ctx: Ctx, ..)` keeps its `Ctx` across every await, and the generated port proxies hold one (`HttpProxy(Ctx)`, `ctx.kv()`), so an async call waiting on a port that never answers pins a dropped runtime, its threads and its in-flight calls until `shutdown`. LC-1's regression test passes only because its fixture calls `rt.port_call` and holds no `Ctx`, which is not the shape of an `#[undra::api] async fn` that takes a `Ctx` or calls a port through a proxy. `undra-query`'s hydration (`kv.get`), persist (`kv().set`) and fetch (`(vt.fetch)(ctx, ..)`) hold a strong `Ctx` across their port awaits the same way ("a step" in ADR-034's sense, so within the rule, but the residual should say so). Dev sessions call `shutdown` (`undra-transport/src/server.rs:289`), so the exposure is embedders and tests that drop without it. | **Documented** in SPEC 5.1 ("Owners and Drop"); characterization test `residual_an_async_call_that_keeps_its_ctx_across_a_port_await_pins_the_runtime_until_shutdown` (pinned after the owner's drop, answered once by `shutdown`), which flips if the residual is ever removed. **Open**: ADR-034's consequence and residual wording (integrator). |
| L1 | Low | `examples/playground/core/src/{counter,bench,lab}.rs` | R10: `Counter` (the first store a reader meets), `Bench` and `Probe` kept a strong `Ctx` field, the pattern E0001's help now tells users not to use, in the app that is meant to show the recommended one. | **Fixed**: `Counter` and `Bench` keep no context (a method runs on the core; `txn` from the prelude, and the change-set goes to the store's owner, ADR-035); `Probe` keeps a `WeakCtx` and upgrades it for the length of `wait`. Playground tests 69/69; bindings unchanged (`undra bindgen --check --docs`). |
| L2 | Low | `crates/undra-runtime/src/runtime.rs:82, 103-105, 251` | `TEST_DRIVER` is a per-thread `bool`: a thread that created **any** `TestRuntime` may write **any** runtime's stores (a threaded `Runtime::new` included) without that runtime's core lock, and the change-set is delivered from the test thread. The record says the driver "counts as the core of its `TestRuntime`". ADR-035 decision 1's wording ("or a `TestRuntime` driver thread") allows it literally. | **Open** (follow-up): record the driving runtime's id (`drive_from_this_thread` keeps "any" for harnesses that build a real `Runtime`). |
| L3 | Low | `crates/undra-signals/src/context.rs:48-58` | `set_write_checker` is first-wins and `clear_write_checker` turns checking off for the whole process; both are `pub` and documented "used by tests". Any crate that calls either before or after the runtime silently disables ADR-035 for every runtime in the process. | **Open**: `#[doc(hidden)]` or a test-only path at the next API pass. |
| L4 | Low | ADR-035 decision 2 (owner `0`); `crates/undra-runtime/src/runtime.rs:237-252` | A signal that belongs to no store but feeds a computed attached to runtime A's store is checked as owner `0` ("any core lock"): a write on runtime B's core is allowed, invalidates A's computed, and commits A's change-set on B's thread without A's core lock (delivered to A's host, unordered against A's core). The ADR chose this; signals shared across runtimes are rare. | **Open** (note). |
| L5 | Low | `crates/undra-runtime/src/runtime.rs:911` (`with_core`) | Two runtimes whose cores call each other's `Ctx::with_core` deadlock (lock-order inversion); `E_REENTRANT` covers only the same runtime. The docs do not say so. | **Open** (doc). |
| L6 | Low | `runtimes/kotlin/.../InprocTransport.kt:252`; `runtimes/kotlin/.../ConnectedCore.kt:372-374` | A stream payload the Kotlin transport cannot read becomes a flag-3 bad request; `failStream` marks it `coreDone`, so the collector never sends `Cancel`, and the core's stream (which did not end) waits for credit until shutdown. Swift fails the stream **and** cancels it (`UndraCore.swift:839-840`, `cancelDeferred`); TypeScript's unknown-flag path does not cancel either (as before the branch). Only a core that sends garbage reaches it. | **Open** (note). |
| L7 | Low | ADR-036 decision 1; `CallError.swift:130`, `core.ts:763`, `ConnectedCore.kt` `streamFailure` | The platforms disagree on what is not supposed to happen: flag 2 on a stream **without** `E` is `.malformed` on Swift but a raw `UndraReplyException(ERROR)` / `UndraReplyError(1)` on Kotlin and TypeScript (generated code has no `fromReply` for such a stream); an undecodable flag-3 body is `UndraProtocolError` (Swift), `UndraTransportError("protocol")` (TS) and `UndraReplyException(BAD_REQUEST)` (Kotlin); a flag-2 body that is not `E` is `.malformed` (Swift), `WireException` (Kotlin), `WireError` (TS), as for a failed call on each platform. The brief cites "ADR-032 Amendment A" for "the three error channels are the same": no such amendment exists yet; it is piece C4b of Amendment C. | **Open** (C4b). |
| L8 | Low | `crates/undra-runtime/src/blocking.rs:93-95` | A blocking job that a worker takes after shutdown began (or after the drop) is skipped without completing its slot. An awaiter outside the runtime (an embedder's `block_on` of `spawn_blocking`) then waits forever; inside the runtime the awaiting task is dropped anyway. Jobs still queued at shutdown were already dropped the same way before the branch. | **Open** (note): completing the slot with a "the runtime is gone" outcome would make it a typed end like `WeakCtx::sleep`. |
| L9 | Low | `crates/undra-runtime/src/blocking.rs:6-9`, `runtime.rs:363`, `testing.rs:10-12` | Stale docs: "wasm and test runtimes run `f` inline" (test runtimes use the real pool since ADR-023), and "fails exactly as in a native debug build" (every build now). | **Fixed**. |
| I1 | Info | `runtimes/kotlin/.../InprocTransport.kt` `close()` | `UndraCore.close()` on an in-process core now blocks until the native shutdown has joined the core's threads (it used to return at once). A sync port implementation that waits for the thread calling `close()` deadlocks; the docs say close is refused from a callback, not that it waits. | Note for the Kotlin docs. |
| I2 | Info | ADR-019 amendment, decision 5 | A failed computed is invisible on the platforms by design: the generated getter returns the last value the host received, with no staleness flag; only the core's log, `poisoned_signals` and a status 2 on a reading call tell. | Note (a platform-visible "stale" bit would be a wire change). |
| I3 | Info | `crates/undra-wire/src/payload/mod.rs` | The unit test sampled five invalid status bytes; the byte fuzz covers the rest only probabilistically. | **Strengthened**: every status byte 0..=255, and a non-UTF-8 `detail` (`InvalidUtf8` at the right offset). |

## The attack on each surface

**1. JNI `shutdown` (R2).** `Java_dev_undra_runtime_UndraNative_shutdown` (`jni_shim.rs:437-442`) has no `unsafe` block:
it calls `session::stop()`, which runs under `guarded`, takes the embedder slot, shuts the global runtime down, clears the
slot and retires the port registrations. Concurrent entries are memory-safe by construction: every API entry clones the
`Arc<Runtime>` it uses (`api::runtime()`), so a shutdown racing a `call`, `call_sync` or `drain` on another JVM thread
leaves that thread a live (shut-down) runtime that answers status 5 or 3; the `JniSink`'s `GlobalRef` is deleted when
the last `Arc` goes, which is the shutdown caller or a concurrent JNI caller, both attached JVM threads (the core,
timer and pool threads are joined before `stop` returns). A second `shutdown` finds no runtime and returns; `init` from
another thread waits on the embedder lock for the shutdown to finish and starts fresh. `JNI_OnUnload` is the same
`stop`. The C ABI and wasm keep their semantics (Swift `shutdown()` is `undra_shutdown`, TS drops the instance), as
ADR-034 decision 8 says. Tests: `shutdown_racing_host_threads_and_a_second_shutdown_answers_each_call_at_most_once`
(new; 5 native runs, and **Miri: 3 passed** with the two existing shutdown tests, `MIRIFLAGS=-Zmiri-disable-isolation`,
nightly 2026-09-29; the only warning is parking_lot_core's integer-to-pointer cast), the JniE2E close race (new). ASan
was not cheap here: on macOS the sanitizer build fails to link (`initializer pointer has no target` for `inventory`'s
initializers with Apple ld); CI's `ffi-asan` job runs `--test abi` on Linux and now includes the new race test.

**2. `WeakCtx` races.** `drive_stream` upgrades with `Weak::upgrade` per item and on End/Error; a send never happens
with a dropped runtime (an upgrade fails once the strong count is 0, and `Drop` starts only then). The gate for "once"
is `calls.remove` on every terminal path (`abort_call`, `task_panicked`, End, Error). `Drop` cannot interleave with a
send: the core loop holds a strong reference for a whole turn, so a drop that lands mid-poll runs after the turn, on
the core thread, and finds the stream still registered. New tests (`weak_ctx.rs`): a busy endless stream with 3,000
credit against the owner's drop at 40 offsets, a finite stream that yields between items against the drop (ended
cleanly 26-27 times and cancelled 13-14 times per run of 40), a busy stream against two concurrent `shutdown`s; each
asserts one terminal item, last, nothing after it, and the release bar of 100 ms. `Ctx::closed()` / `WeakCtx::closed()`
are per runtime instance (the `Lifeline` is created with the runtime; a reload is a new runtime with a new id from
`NEXT_RUNTIME_ID`, never reused), resolve once with the first reason, and cannot fire early for a reloaded runtime; a
`Closed` cannot remove another's waker (registration is refused after `close` drains the slab).

**3. The write checker (ADR-035).** The check is a runtime-id comparison: `write_allowed(owner)` looks for `owner`
in `HELD`, a thread-local `Vec<u64>` of the runtimes whose core lock the thread holds (pushed by `CoreGuard`,
`HeldMark`), with ids from a monotonic process counter. Thread identity is never consulted, so an OS thread id reused
after A's core thread exited cannot alias, and B's core is refused for A's stores (`ow2_*`). "First checker installed
wins" is harmless for the runtime (it installs one function, the same for every runtime) and harmful only if someone
else installs or clears one (L3). The `TestRuntime` driver exemption is not per runtime (L2), and the owner-`0` rule has
the cross-runtime hole of L4. Cost: `cargo test -p undra-bench --test budgets --release`: `signals/set_attached`
**8.7 ns** median (budget 65; the bench runs on a driver thread, so all three thread-local reads are paid), no
allocation and no lock on the check (`OnceLock::get`, two `try_with`s, a `Vec::contains`); the delivery path through
the `RUNTIMES` registry (a `parking_lot` read lock, a SipHash lookup, a `Weak::upgrade`) is in
`signals/changeset_100/runtime`: **2.19 us**, budget 11 us, baseline 2.18 us. `deliver_from` routing: a change-set
for B's store can only be produced under B's core lock (`with_core` on B, a dispatched call or task of B), so it is
ordered with B's own by the core lock and the store's delivery lock, and reaches B's host; ADR-031's coalescing is on
the platform side and sees the same per-store order.

**4. Panicking computeds.** `encode_computed` (`store.rs`) wraps `(slot.encode)(out)` in `catch_unwind` with
`AssertUnwindSafe` over the computed's closure and a scratch `Writer` cleared before each use, so a partial encoding
never reaches a change-set. The graph stays consistent across the unwind: `Computed::recompute` keeps the node dirty
(`RestaleOnUnwind`), pops its cycle-detection frame (`Recomputing`'s `Drop`), and the store slot's dirty bit, claimed
before the evaluation, is re-armed by the next invalidation; nothing is skipped on the next write. New test
`a_no_op_write_to_a_failed_computeds_input_evaluates_it_again_without_a_second_report`: writing the input the value it
has is a change for the graph (no equality check), so the computed is evaluated once more, still fails, stays held back,
is **not** reported again, and the input is delivered; an unrelated write evaluates nothing. Dependents fail with it and
are reported once each (`pc1_a_computed_that_reads_a_failing_one...`). The platforms keep the last value (I2).

**5. Wire (ADR-036).** ADR-036 decision 6 and Amendment C: no envelope version bump before publication (ADR-033's
argument), ADR-036 and ADR-037 in one wire revision; `VERSION = 1` matches. No schema hash rule changed: the stream of
`Result<T, E>` is recorded as `Result<Stream<T>, E>` and the generated code is identical to a fallible opening's.
Coverage: proptests round-trip `StreamFailure` with the three statuses and arbitrary strings, every strict prefix
fails; the byte fuzz decodes random bytes as `StreamFailure`; the unit test now covers all 256 status bytes and a
non-UTF-8 `detail` (I3). The malformed item: Kotlin's transport reports it to the stream's collector as a flag-3 bad
request and ends the flow (the core's side stays open, L6). The platforms' handling of the impossible cases differs
(L7).

**6. R6 and the E0065 panic.** Where an off-core write can happen and where its panic goes: a blocking-pool closure
runs under `guard::guarded` and the panic is re-raised in the awaiting task (status 2 for a call); a dispatched call or
task is guarded as before; every C-ABI and JNI entry runs its body under `guarded`, and the natives without their own
`guarded` (`cancel`, `streamCredit`, `observe`, `release`, `timerFired`, `shutdown`) call `api` functions that are; the
timer thread runs wakers only; a user `std::thread` unwinds itself (no foreign frame). No path unwinds through an
`extern "C"` or `extern "system"` frame. On wasm (`panic = "abort"`) a refusal would trap the core, but every wasm entry
that runs user code holds the core lock, so the refusal is unreachable there; the inline blocking runner runs its
closure under the core lock, so wasm allows a write that native refuses (test runtimes use the real pool and see the
native rule). While a thread unwinds the check is skipped (ADR-035 kept the rule), so `Drop` code during an unwind
writes unchecked.

**7. Macros and bindgen.** `impl Stream<Item = Result<T, E>>` in all three positions (plain, inside `Result<.., E>`,
`async`) maps to `KType::Result(Stream(T), E)`; `a_stream_of_results_has_the_schema_of_a_fallible_opening` compares the
recorded `TypeRef` of the four new methods with the existing `try_counts` and a literal, which is the right check (the
hash is a function of the canonical schema). The playground's `Probe.ticks_then_fail` generates exactly the code a
fallible opening does in TS, Kotlin and Swift (item type `UInt32`, error through `LabError.fromReply` /
`mapped(streamFailure:domain:)`). E0005's new message: code, what (names both enums), why, fix, docs link, on the
return type (`tests/ui/e0005_stream_item_error_mismatch.stderr`); E0065's: code, what, note, help (names
`ctx.with_core` and `try_set`), docs (`tests/golden/diagnostics/E0065.txt`).

**8. Contracts.** `bash contract-tests/run-all.sh`: **18 x 3 = 54/54** before and after the fixes (no scenario was
added; S07.6, S07.7 and S17.7 are steps). S17.7: see M1; the Stress generator sleeps 10 ms per tick on the `Timer` port (the
JVM default adapter on Kotlin, the harness's `TimerAdapter` on Swift) and reads the Clock at each, so a surviving
generator that kept being woken would make about 20 Clock reads in 200 ms, but neither platform routes an old core's
calls (or the timers it set) to anything counted after close; the window was a real duration over an unreachable
signal. The Swift `ManualClock` fix (milliseconds to nanoseconds) changed what
`monotonic_ns` reports; only `Stress` reads `monotonic_ns` (grep of the playground core and `undra-query`), so no
earlier scenario depended on the unit, and every Swift S04 and S13 timing step still passes.

**9. Everything else.** See the verification section. `pub` items added by the branch carry docs; no `println!` in
library crates (the one in the new finite-stream race test is a test's own output); no new dependency (`slab` and
`parking_lot` were already in `undra-runtime`).

## Verification (after the fixes, `b3f4646`)

* `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy -p undra-ffi --target
  wasm32-unknown-unknown -- -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` and `cargo build
  -p undra-ffi -p playground-core --target wasm32-unknown-unknown`: clean.
* `cargo test --workspace --no-fail-fast`: 133 suites, **2,345 passed**, 0 failed, 11 ignored (2,339 at `480a0f3`,
  reproduced before any change; the six new tests are the three `once_*` races, the residual and the no-op write in
  `undra-runtime`, and the shutdown race in `undra-ffi`). Release: `write_context` 9 and `write_checker` 8 (the CI step),
  `weak_ctx` 19 and `computed_isolation` 3 in five consecutive runs.
* Miri (nightly, 2026-09-29): `MIRIFLAGS=-Zmiri-disable-isolation cargo +nightly miri test -p undra-ffi --test abi --
  shutdown_racing shutdown_is_idempotent_and_init_can_follow shutdown_with_calls_in_flight`: 3 passed (22 s). ASan: not
  run, see surface 1.
* Bench (release, on a machine other agents were loading): `budgets` 6 passed, 1 ignored (`signals/set_attached`
  8.5 ns, budget 65 ns; `signals/changeset_100/runtime` 2.26 us, budget 11 us, 2.19 us before the fixes;
  `dispatch/call_sync/add` 46.8 ns); `stress` 11 passed, 1 ignored; `sync_alloc` 2 and `commit_alloc` 5 passed.
* TypeScript runtime: 983 passed (28 files). Kotlin runtime over the real JNI library (the playground core,
  `UNDRA_NATIVE_LIB_DIR=examples/playground/build/host`): 522 cases in 31 suites, 0 failed, 1 skipped (the "no native
  library" case). Swift runtime: 446 tests, 0 failures.
* `crates/undra-ffi/tests`: wasm 19 (raw) + 10 (TS runtime) passed; C smoke and lifetime ok; Swift ok; JNI E2E 14 passed
  (13 + the new close race; four runs).
* `contract-tests/run-all.sh`: **18 x 3 = 54/54**, and the Kotlin and Swift columns twice more after the S17.7 change.
  Mutant runs as in M1 (the mutant is not committed).
* `undra bindgen -C examples/playground --check --docs`: up to date, schema hash `0xddcdea47fa95a8d4` (the playground
  edits change no schema). `node site/scripts/build-all.mjs`: nothing to regenerate.

## Open items for the integrator

1. M2: amend ADR-034's consequence ("a call no longer pins the runtime") and residual to name a `Ctx` held across an
   await (an async method's parameter, the generated port proxies); decide whether generated proxies and the injected
   `Ctx` of async functions should become weak in a later ADR.
2. L2 (per-runtime test driver), L3 (`set_write_checker` / `clear_write_checker` exposure), L4 (owner `0` across
   runtimes), L5 (`with_core` lock order), L8 (a skipped blocking job leaves its awaiter waiting): follow-ups, none
   blocking.
3. L6 and L7 belong to C4b (the Kotlin and TypeScript failure model, "wire errors under one base"), which is where the
   missing "ADR-032 amendment" lives.
4. I1: say in `UndraCore.close()`'s Kotlin docs that close now waits for the native shutdown.
5. `.10x/status.md` / `handoff.md`: `undra_stats_json` with no runtime gained `runtime_threads` (SPEC 6).
