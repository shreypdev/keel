# SDE — runtime lifecycle: WeakCtx, the write rule, computed isolation, typed stream ends (wt/runtime-lifecycle, 2026-10-01)

Track A of the v1.x program: ADR-034 (A1), ADR-035 (A2), the ADR-019 amendment (A3) and ADR-036 (A4), from
the gap audit `.10x/specs/2026-10-01-v1x-gaps.md` (LC-1…LC-3, PA-6, OW-1, OW-2, PC-1, ST-1/ST-2). The
audit's reproductions are the regression tests. One worktree, no push, no merge; `main` merged in before
the final verification.

## A1. ADR-034: `WeakCtx`, and a runtime ends when its owner lets go

* `undra-runtime`: `WeakCtx` (`downgrade`, `upgrade() -> Result<Ctx, Gone>`, `is_alive`, `closed`, `sleep`),
  `Gone { ShutDown, Dropped }`, `Ctx::closed()`. A `Lifeline` (an atomic state plus wakers) is closed by
  `shutdown` and by `Drop`, so `closed()` and a `WeakSleep` end typed either way; the first reason wins (a
  shut-down runtime that is then dropped still says `ShutDown`).
* The runtime's own call and stream tasks hold `Weak<Runtime>`; `drive_stream` upgrades per item. `Drop`
  answers every in-flight call and open stream once, like `shutdown` ("the runtime was dropped"), and never
  joins its own core thread. Event subscribers receive `&Ctx` as an argument (`EventHandler =
  Box<dyn Fn(&Ctx, &[u8])>`), so a subscriber needs to capture nothing. `stats` reports `strong_refs`.
* `undra-query`: the hydration hook, fetch, persist, replay, GC and retry tasks and `QueryHandle` hold a
  `WeakCtx`; `backoff_sleep` returns `Err(Gone)` when the runtime goes.
* `undra-macros`: a `WeakCtx` store field restores by downgrading the restore context; E0001/E0013 help texts
  name `WeakCtx`. `undra-ports`: the event helpers pass the `Ctx`.
* Kotlin: `UndraNative.shutdown()` (a JNI native registered next to the others, `session::stop()`);
  `InprocTransport.close()` calls it when the load succeeded, so a later `UndraCore.load` in the same process
  works. The playground's `Stress` store and its generator hold a `WeakCtx` (the periodic-task idiom).
* Regression tests (fail with the tasks pinning the runtime): `crates/undra-runtime/tests/weak_ctx.rs` (15:
  `lc1_*` an open endless stream / an async call blocked on a port do not pin a dropped runtime and are answered
  once; `lc2_*` a periodic task, a subscriber and a store with a `WeakCtx` field do not pin it; shutdown
  ordering; a drop from the core thread), `crates/undra-query/tests/lifecycle.rs` (3, `lc3_*`), Kotlin
  `InprocTransportTests` (close shuts the native core down once and a new load starts fresh; close from a core
  callback is refused), `NativeTests` (the native's shape, load and close over the real JNI library), contract
  S17.7.

## A2. ADR-035: a write off its runtime's core is refused in every build

* `undra-signals`: the write checker is asked about the store's owning runtime (`StoreCell::owner`, set by the
  object table before the handle); a refused write panics with the E0065 teaching message in every build, or
  `Signal::try_set` / `try_update` return `WriteError::OffCore { owner }`; `can_write()` asks first. A refusal
  is reported to the sink (`ChangeSink::off_core_write`) before the panic, so the runtime logs it and counts
  `off_core_writes`. Commits name their owner (`deliver_from`), so a change-set reaches the store's runtime.
* `undra-runtime`: `Ctx::with_core(|| ..)` runs a closure under the core lock from a host thread (one
  change-set; `E_REENTRANT` from a callback or the core). The test driver thread counts as the core of its
  `TestRuntime` (review L3 closed); `testing::unchecked_writes` lifts the check on one thread.
* E0065 is in the catalogue (SPEC 12, `diag.rs`, the audit's emitter list, the golden
  `crates/undra-signals/tests/golden/diagnostics/E0065.txt`, the error-codes page).
* CI runs `crates/undra-runtime/tests/write_context.rs` and `crates/undra-signals/tests/write_checker.rs` in
  release too (the audit's misbehaviours were release-only).
* Regression tests: `write_context.rs` (9: `ow1_*` a plain thread and a blocking-pool thread, `ow2_*` a write
  to runtime A from runtime B and `with_core` routing it to A, `l3_*`, `l4_*`), `write_checker.rs` (8). With the
  check skipped in release, 5 of `write_context.rs` fail.
* Bench: `signals/set_attached` (a write to an attached, observed signal, the checked path) budget 65 ns.

## A3. ADR-019 amendment: a panicking computed poisons only itself

* `undra-signals`: a computed that panics while its store commits or is observed is held back alone (typed
  health per signal: `StoreCell::failed_signals`, `is_failed`); the write that made it fail succeeds and its
  own slot is delivered; the store and the runtime keep working. It is evaluated again when one of its inputs
  changes, and a recovered computed is delivered in full. One report per failure (`computed_failed`, an ERROR
  log), `poisoned_signals` in the stats. A call that reads it gets the ordinary panicked reply (status 2), which
  every platform maps already. An observe leaves a failing computed out instead of failing the store.
* Regression tests (fail before: every write answered status 2 and no change-set was delivered):
  `crates/undra-runtime/tests/computed_isolation.rs` (2), `delivery_integrity.rs` `pc1_*` (3).

## A4. ADR-036: typed stream error items

* Wire: flag 2 carries only the stream's own `E`; flag 3 is a `StreamFailure { status, message, detail }`
  (status 2 panic, 3 cancelled, 5 bad request; any other byte is `InvalidTag("StreamFailure.status")`). The
  "cancelled: …" string path is gone. Four new vectors in `contract-tests/wire-vectors.json` (Swift copy synced,
  Kotlin vectors regenerated).
* Rust: `impl Stream<Item = Result<T, E>>` (plain, inside `Result<.., E>`, or `async`) ends a stream with its
  typed error part-way; E0005 teaches an item error that differs from the opening's. The schema of a stream of
  `Result<T, E>` is the schema of a fallible opening (asserted in `macros/tests/objects.rs`), so the hash does
  not move for the shape. The runtime sends flag 3 for a restore or drop (cancelled), a panicking producer
  (panic, with the backtrace as detail) and a malformed item.
* TS, Kotlin and Swift decode flag 2 as the generated error type and flag 3 as the core's failure (TS
  `UndraReplyError`, Kotlin `UndraReplyException`, Swift `UndraCallError.cancelledByCore` / `.panicked` /
  `.refused`); a transport-level malformed item is a flag-3 bad request.
* Contract: S07.6 (`ticks_then_fail(5, 3, 7)` yields 0,1,2 then `LabError.Rejected { code: 7 }`), S07.7 (a
  restore cancels an open stream with an error type: cancelled by the core, never the stream's error or a wire
  error), on all three platforms.

## Verification (after merging `main`)

* `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy -p undra-ffi
  --target wasm32-unknown-unknown -- -D warnings`, the wasm32 builds of the core crates and `cargo doc` with
  `-D warnings`: clean.
* `cargo test --workspace`: 2339 passed, 0 failed, 11 ignored (2236 on the branch's base before the piece, 2279 with all four pieces before the merge; the merge
  adds main's own tests).
* Release: `write_context` 9 and `write_checker` 8 (the CI step), and `weak_ctx` 15, `computed_isolation` 2,
  `streams` 12: all pass.
* TypeScript runtime 983 passed; Kotlin runtime over the real JNI library 522 cases, 0 failed, 1 skipped (the
  "no native library" case, skipped because the library loads); Swift runtime 446 tests, 0 failures.
* `crates/undra-ffi/tests`: wasm 19/19, C smoke and lifetime ok, Swift ok.
* `contract-tests/run-all.sh`: 18 x 3, every cell pass.
* Bench (release, on a loaded machine): `budgets` and `stress` pass; `signals/set_attached` 8.5 ns (budget
  65 ns), `stress/stream/items_x1000` 34.2 us (budget 180 us). The ffi allocation gates (`sync_alloc`,
  `commit_alloc`) pass in release: neither the weak task handles nor the write check allocate on the hot path.
* `runtimes/swift/scripts/sync-vectors.sh --check`, `gen-vectors.py --check` (Kotlin) and `undra bindgen -C
  examples/playground --docs --check`: up to date (playground schema hash 0xddcdea47fa95a8d4 after the merge).
* The error-codes page and the site indexes are regenerated (`build-errors.mjs`, `build-all.mjs`).

## Deviations and notes for the integrator

* `round_cap_hit` still counts per current-or-global runtime, not per owner (out of scope).
* `observe` isolates a panicking computed too (the amendment speaks of commits; an observe that failed the
  whole store would have been the same bug).
* `Gone` is in the facade prelude next to `Ctx` and `WeakCtx`.
* Effects and RX-3 are not part of this piece.
* The first checker installed wins (`set_write_checker` is a `OnceLock`); the runtime installs it.
* `undra-macros` and `undra-ports` changed as far as ADR-034's signatures required (the event helper, the
  `WeakCtx` field).
* S17.7 is native-only (a trapped wasm core has nothing to shut down or reload). On Kotlin the 200 ms window
  after `close()` cannot fail (the transport detaches first), so S17.7 also requires the fresh core's Clock to
  stay quiet for 200 ms, where a surviving task would show.
* The Swift contract harness's `ManualClock` answered `monotonic_ns` in milliseconds; fixed (nanoseconds).
* The playground's `Counter`, `Probe` and bench stores still keep a strong `Ctx` field; `shutdown` breaks those
  cycles (ADR-023), and none of them runs a task. Converting them is an R10 polish item.

## After review

The adversarial review (`.10x/reviews/2026-10-01-runtime-lifecycle-review.md`) changed what S17.7 proves. The
sentence above that the fresh core's 200 ms Clock window is "where a surviving task would show" was wrong: a task of
the old runtime calls through the old runtime's host (the detached Kotlin callbacks, or C-ABI registrations the
shutdown retired), never through the fresh core's, so neither window could fail; a shutdown mutant that only released
the global slot passed all 18 scenarios on Kotlin and Swift. With no core loaded, `undra_stats_json` (JNI
`statsJson()`) now reports `runtime_threads`, and both runners require 0 after every close; the mutant fails S17 on
both. The review also converted the playground's `Counter`, `Bench` (no context) and `Probe` (a `WeakCtx`), and
recorded in SPEC 5.1 that a `Ctx` held across an await (an async method's parameter, a port proxy) pins the runtime
until the await completes (the `lc1_*` call fixture holds none).
