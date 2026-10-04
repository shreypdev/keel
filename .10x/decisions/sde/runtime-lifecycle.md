# SDE - runtime lifecycle: WeakCtx, the write rule, computed isolation, typed stream ends (wt/runtime-lifecycle, 2026-10-01)

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

## Merge of `main` at `c186665` (2026-10-01, SDE, "bringing the branch across")

The branch merged `42fdd42` earlier. `main` then gained dev-loop (ADR-051), parity (ADR-032 amendment A for Kotlin and
TypeScript), the `cas_update` helper, CI pinned to Rust 1.98.1 and the schema-docs pin fix. The ADR-034 amendment
landed first as its own commit (`4b22ae1`); `git merge main` followed. Rust merged without a textual conflict
(`runtime.rs`, `object_table.rs`, `lib.rs`); the branch adds no `fetch_update`. What conflicted, and what came from each
side:

* **`runtimes/kotlin/.../ConnectedCore.kt`.** `close()`: the transport's `checkClose()` (ours) and
  `shutDown(null, ClosedReason.REQUESTED)` (dev-loop's connection state). `shutDown`: dev-loop's signature, our KDoc on
  the order (fail pending, cancel scope, then the blocking transport close). Stream failure: our `streamFailure` (flag 3
  becomes `UndraReplyException(status, body)`) next to parity's `onMalformed` (an item the transport cannot read becomes an
  `UndraProtocolException`, generated as `Malformed`). **Decision:** an undecodable flag-3 body is now an
  `UndraProtocolException` too (it was `UndraReplyException(BAD_REQUEST)`, which would have been generated as `Refused`, a
  case that says the core refused the call); Swift (`UndraProtocolError`) and TypeScript (`UndraTransportError("protocol")`
  maps to `Malformed`) already did this, so the three platforms agree where review item L7 said they differed.
  **L6 (Kotlin did not cancel the core's stream after an unreadable item) is fixed where it now belongs**:
  `ConnectedCore.onMalformed` cancels a streaming entry from the delivery thread (a native call from the callback is
  refused), like Swift's `cancelDeferred`. Tests: `InprocTransportTests` (the unreadable item cancels exactly its stream and
  nothing else; the core's own flag-3 ends cancel nothing), which fail with the cancel removed (mutant run).
* **`InprocTransport.kt`.** Parity's `onMalformed` for an unreadable stream item (ours made it a synthesised flag-3 failure
  and cancelled in the transport; that moved up to `ConnectedCore`); the `close()` KDoc and `native.shutdown()` stay;
  "already loaded" now says close it first (it said "cannot be unloaded", untrue since ADR-034).
* **`UndraCore.kt`.** Our `close()` KDoc (it waits for the native shutdown) with parity's `UndraTransportException`
  wording and the `E_REENTRANT` reply a close from a callback throws; `load` and `stream` docs corrected (a second
  in-process load works after `close()`; a stream ends in the vocabulary of a failed reply).
* **The status-5 mapping and the stream mappings (Kotlin and TypeScript).** Parity's `mappedStream` guessed from the text
  of a flag-2 body ("cancelled: " is a cancellation, anything else a panic), the stop-gap ADR-036 exists to remove.
  Replaced, in both runtimes, by what Swift's `mapped(streamFailure:)` already does: a flag-3 item maps by its status, as a
  failed reply does (3 `CancelledByCore`, 2 `Panicked` with message and backtrace, **5 `Refused` with the reason**); a
  flag-2 item decodes as the stream's `E` (`Malformed` if it does not, and `Malformed` when the stream has no `E`);
  nothing is read from text. Tests: Kotlin `CallErrorTests`, `InprocTransportTests`, `GoldenFullTests` (a new refused
  case), TypeScript `call-error.test.ts`, the generated-code fixtures `kotlin-run/full/FullTest.kt` and `ts-run/full.mjs`
  (the String items became flag-3 failures; a status-5 case was added).
* **Contract runners.** `World.kt` has both sides' fields (`portCalls`, `options`, `unhandled`); `S17Panic.kt` runs main's
  `UndraCore.current == null` check after S17.6 and before our S17.7 (which loads a fresh core and so makes `current`
  non-null again), and keeps the `runtime_threads == 0` check; the Swift runner likewise. S07.7 on Kotlin and TypeScript
  now expects `UndraCallError.CancelledByCore` (generated code maps it since parity), and the TypeScript step restores
  through the public `core.snapshot()` / `core.restore()` (parity removed the harness's raw wasm exports), so S07 no longer
  needs `bootRaw`.
* **Docs.** `docs/SWIFT_ERRORS.md` was deleted on main: our "How a stream ends" section went into `docs/ERRORS.md` as one
  table for all three platforms, and its stale "the mapping will land with ADR-036" paragraph was replaced. SPEC 3.7 now
  names the `UndraCallError` case per status on all three platforms, SPEC 17 the `mappedStream` rows, SPEC 6.1 that Kotlin's
  close waits. `scenarios.md`, the three NOTES, both runtime READMEs and `.10x/decisions/sde/_index.md` keep every line
  from both sides.
* **Generated code.** `undra bindgen -C examples/playground --docs` rewrote the two conflicted files
  (`Objects.kt`, `objects.ts`); everything else was already right. **The playground schema hash is
  `0xddcdea47fa95a8d4`** (main had `0x04d2adf769c58b9f`: the branch's `Probe`/`Lab` additions moved it; parity moved
  nothing), and `bindgen --check --docs` is up to date. No golden changed under `UPDATE_GOLDEN=1`. The site pages
  (`build-errors.mjs`, `build-all.mjs`) regenerate to nothing.

Read end to end after the merge (`core.ts`, `UndraCore.kt` with `ConnectedCore.kt`, `UndraCore.swift`):

* a reconnect still fails what is in flight with the typed `Unavailable` (TypeScript `#reconnecting` fails calls and
  streams with `UndraTransportError("closed")`; Kotlin `onReconnecting` with `UndraTransportException(CONNECTION_LOST)`;
  Swift `onReconnecting` with `UndraTransportError.connectionLost`), and the playground interop runs on TypeScript and
  Kotlin assert "failures seen ... all typed Unavailable";
* a flag-3 failure reaches the stream's consumer as its typed case on every platform (tests above, S07.7 on all three);
* `onError` stays silent for a drop the connection state reports (`report` / `isConnectionDown` in all three; a stream's
  failure never goes through `onError` at all).

Not changed, as the review recorded them: L2 to L5, L7's remaining differences, L8; TypeScript's unknown-flag path still
does not cancel the core's stream (the L6 residue). ADR-032's amendment A still reads "until then a stream's flag 2 is
guessed"; it is history, and `docs/ERRORS.md` is the current statement.

Verified on the merged tree (`UNDRA_REQUIRE_TOOLCHAINS=1`, `tsc` on `PATH`; see the integrator report for the counts):
`cargo fmt --check`, `clippy --workspace --all-targets -D warnings`, `clippy -p undra-ffi --target wasm32-unknown-unknown`,
`cargo doc` with `-D warnings`, `cargo test --workspace`, the release regressions, `schema_docs -- --ignored`, Kotlin
(with and without the JNI library), TypeScript tests and typecheck, Swift, the wasm, C and JNI harnesses, the contract
grid, both interop runs, the Android `assembleDebug`, the playground web tests and build, the budgets, stress and
allocation gates, the site link check.

### Later merges of `main` (2026-10-01): dev-loop/parity checkpoint, React Native, Android adapters

`main` moved three more times while the first merge was verified (`18cf0b8`: site pages and a Kotlin test-lambda fix for
CI's kotlinc 2.0.21; `e518653`: the React Native runtime; `429fb9f`: the Android adapters). All three merged without a
textual conflict, and none needed a decision beyond these:

* **React Native (`runtimes/rn/@undra/react-native`).** `NativeTransport` decodes no stream item: it forwards the
  `StreamItem` record to `UndraCore` verbatim, so flag 2 and flag 3 are handled by the TypeScript runtime's decoding
  already, and nothing needed bringing in line. Four tests in the package's `npm test` now pin the pass-through (an end
  item, flag 2 as the stream's `E` untouched, flag 3 as the failed reply of its status mapping to `CancelledByCore` /
  `Panicked` / `Refused`, an undecodable flag-3 body as a protocol error mapping to `Malformed`): 45 tests, typecheck
  clean, `test:contract` 17 passed, S17 skipped (app-tested), as on `main`.
* **Android adapters.** The playground `UndraApp.kt` keeps their `AndroidPlatformDefaults.install` right after
  `UndraCore.load`; the branch changed nothing there. `docs/ERRORS.md` has the adapter-failure rows and the stream table
  side by side; the decision index, site pages and indexes regenerate to nothing.

Final counts on the tip (`UNDRA_REQUIRE_TOOLCHAINS=1`): `cargo test --workspace` 2,400 passed, 0 failed, 11 ignored
(128 suites); TypeScript runtime 1,102 (32 files) and typecheck clean; Kotlin 612 cases, 0 failed, 2 skipped under
brew's kotlinc 2.4.20 and under CI's 2.0.21; Swift 480; wasm 19 + 24; JNI E2E 14; the contract grid 18 x 3 = 54/54;
both interop runs; the playground web 112 and build; the Android `assembleDebug`.
