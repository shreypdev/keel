# SDE - frame-coalesced delivery (wt/coalesce, 2026-09-30)

Implements ADR-031 (accepted with the integrator's D1 conditions (a)-(e), appended to the ADR) in the
three platform runtimes. No wire, C ABI or wasm ABI change; `undra-signals` and `undra-runtime` are
untouched. Decision 6 (`no_coalesce` through the schema) is its own commit.

## What changed

**TypeScript** (`runtimes/ts/@undra/runtime`): `src/mirror.ts` (the fold, compaction, frame schedule
`scheduleFrame`, counters `stats()`, `addDrainListener`, `queueFlush`, `RegisterOptions.noCoalesce`),
`src/core.ts` (replies queue a flush before they settle, `callSync` flushes before it returns, resync
sends `Observe` on, `AttachOptions.mirror`, `UndraStats.mirror`), `src/object.ts` (`StoreOptions`),
`src/worker.ts` + `src/transport/wasm-worker.ts` + `src/transport/worker-protocol.ts` (protocol 2: one
`envelopes` message per worker task; the host reads both shapes; a v1 host gets single envelopes),
tests `test/coalesce.test.ts` (30) and two updated expectations (`mirror.test.ts`: a valid patch;
`react.test.ts`: waits for the frame).

**Kotlin** (`runtimes/kotlin/undra-runtime`): `runtime/.../Mirror.kt` (rewritten: parsed entries in
parallel arrays under one lock, the same fold as TS with an allocation-free slot index, compaction with
weights so dropped entries stay counted, resync from the main thread, `register(handle, noCoalesce, apply)`
next to the two-argument form, `stats()`, `addDrainListener(): AutoCloseable`, public `flush()`),
`FramePacer.kt` (public `fun interface FramePacer`; internal `PacedFramePacer`, one `undra-frame` daemon on
a 16.67 ms grid posting to the main thread), `MirrorStats.kt` (`MirrorStats`, `DrainStats`), `LoadOptions.kt`
(`MirrorOptions(framePacer, maxPendingEntries, maxPendingBytes)`, `LoadOptions.mirror`), `ConnectedCore.kt`
(a reply posts an immediate main-thread drain before completing the call; `callSync`/`construct` on the main
thread drain before returning; `call` also drains when it resumes on main, for dispatchers that overtake a
posted message), `UndraStore.kt` (`noCoalesce`), `UndraStats.kt` (`mirror`), `UndraDispatchers.kt` (the
Android main-thread check compares threads instead of reflecting per call). New module
**`android-adapters`** (`com.android.library`, namespace `dev.undra.android`, AGP 8.7.3, included by
`settings.gradle.kts` only when an Android SDK is found) with `ChoreographerFramePacer` (condition (a)); the
playground Android app installs it. Tests: `CoalesceTests.kt` (41 cases), `MirrorTests.kt` updated,
`ManualMainThread.kt` (`ManualFramePacer`).

**Swift** (`runtimes/swift/UndraRuntime`): `Core/Mirror.swift` (rewritten: parsed entries as
`ArraySlice`s, the fold, compaction, awaiting-full-value enforced on arrival, registrations snapshotted
once per round, `register(_:noCoalesce:_:)`, `stats()`, `addDrainListener -> DrainListenerRegistration`,
`withImmediateDrain` for main-thread `callSync`/`construct`/`observe`, `drainBeforeResuming` for replies),
`Core/FrameScheduler.swift` (`CADisplayLink` on iOS, tvOS and visionOS, paused while idle, unpaused through
one main-actor hop from other threads; the main-actor hop on macOS; a test seam), `Core/MirrorStats.swift`,
`Core/UndraCore.swift` (reply drain on the main queue before the continuation, resync, `stats().mirror`,
scheduler invalidated at shutdown), `Core/UndraObject.swift` (`UndraStore.init(core:handle:noCoalesce:)`),
`Core/LoadOptions.swift` (`maxPendingEntries`, `maxPendingBytes`), `Core/UndraStats.swift`, README. Tests:
`CoalesceTests.swift` (37), `FakeTransport.swift` (`ManualFrameScheduler`), `MirrorTests.swift` (one test
moved to merged behaviour).

The Kotlin and Swift runtime work was drafted by two helper agents from briefs that pinned the shapes, then
reviewed, completed (the `android-adapters` module and the playground wiring are mine) and verified here.

**Schema and codegen (decision 6)**: `undra-meta` (`SignalDef.no_coalesce`, `#[serde(default,
skip_serializing_if)]`; `SignalMeta.no_coalesce`), `undra-macros` (records the attribute in the meta),
`undra-bindgen` (`model::no_coalesce_ids`; the three store templates pass the ids: TS `super(core, handle,
{ noCoalesce: [..] })`, Kotlin `UndraStore(core, handle, noCoalesce = setOf(..u))`, Swift
`super.init(core:handle:noCoalesce:)`, only for stores that have such signals, so every other generated
file is byte-identical), the `stores` golden's `Clock.now` is `no_coalesce`, and
`tests/schema_hash.rs` pins the pre-ADR hash of all nine golden schemas.

**Playground** (`examples/playground/core/src/stress.rs`): a minimal `Stress` store - `value: u64`
(Firehose), `#[undra(no_coalesce)] progress: u32` (Progress), `burst(mode, n)` that commits `n`
transactions in a tight loop, no `ctx.txn`. Bindings regenerated with `undra bindgen --docs`
(schema hash `0x3fb88e5bb974203c`). The S1b stress screen (stress-bench design §8) can grow this store
(`StressMode` gains `Churn`/`Board`).

**Contract scenario S18 coalesced burst** (`contract-tests/scenarios.md`, `check.sh`, `run-all.sh`, the
three runners). Kotlin's `flushMainThread()` now drains the mirror on the main thread (`mirror.flush()`),
and every look of a Kotlin wait (`awaitUntil`, `awaitEq`, `holdsFor`) drains first: with the paced 16.67 ms
grid a store lags the core by up to a frame, and S12 read `fetching == false` before the change-set that set
it had been applied, then refetched into a fetch still in flight (the core deduplicates, so no 503 came).

**Docs**: SPEC §2.2/§2.3/§4.3 (`no_coalesce` in the schema), §11.1 (the exact rules), §17 (options,
counters, listener per language); `docs/HIGH_FREQUENCY.md`; ADR-031 consequences carry the measurement.

## Decisions taken while implementing (not spelled out by the ADR)

1. **"max(4,096 ops, 1 MiB)" read as both bounds**: a merged patch is dropped only when it has more than
   4,096 operations *and* more than 1 MiB of op bytes. The cap is checked by compactions only (memory is
   what it bounds); a drain applies whatever merged patch it holds.
2. **The backlog bound wins over `no_coalesce`.** A compaction folds every key, opt-out ones included;
   otherwise a flooding `no_coalesce` signal would make the backlog unbounded (decision 3 vs decision 6).
   Documented in SPEC §11.1 and the docs page.
3. **Compaction amortisation**: after a fold the next one waits until `max(bound, 2 × size)`, so a queue
   that stays near the bound after folding (many distinct keys) does not fold on every enqueue.
4. **A patch shorter than its count** cannot be merged; the key is dropped and resynchronised (an error is
   reported) instead of being passed through to fail in the decoder.
5. **TS frame backstop**: `requestAnimationFrame` plus a 100 ms timer, whichever comes first, so a tab
   hidden after the request (or an off-screen iframe that gets no frames) still drains. Visibility other
   than `"visible"` (`hidden`, the legacy `prerender`) uses a zero-delay task.
6. **TS read-your-writes from a microtask**: the reply arrives inside a wasm import in `wasm-main` (the core
   lock may be held), so draining there would run subscribers inside a core callback; a flush microtask
   queued *before* `resolve` runs before the caller's continuation, which is the same guarantee.
7. **Observe waiters** (worker, remote) drain from a microtask, not the frame, so `create()` never waits for
   a frame or for a hidden tab.
8. **Resync** sends only `Observe(on)` (re-observing an observed signal re-sends its value, §5.5), without a
   waiter, so a slow socket cannot turn it into an unhandled timeout.
9. **Worker protocol version** travels in `init`; the worker batches only for a host that announced 2.
10. **Kotlin `register` is two overloads** (the two-argument one kept, the three-argument one without a default):
    the golden `full` case's test double overrides the two-argument form. `UndraStore` uses it when it has no
    `no_coalesce` signal.
11. **Swift extra rounds only for work the drain caused** (entries queued on the main thread during it, or a due
    resync); change-sets from other threads during a drain wait for the next frame, so a firehose cannot hold one
    drain for 1,000 rounds. Kotlin and TS take everything queued in further rounds (TS has one thread; Kotlin's
    rounds are bounded by the same cap). Both are within the ADR (it does not say which).
12. **Swift reply drain on the main dispatch queue** (`DispatchQueue.main.async` + `assumeIsolated`), not
    `Task { @MainActor }`: strictly FIFO with the continuation's main-actor job.

## Verification (before merging `main`; re-run after it, below)

| Check | Result |
|---|---|
| `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | 2,117 passed, 0 failed, 9 ignored (2,110 before: meta +1, macros +1, bindgen +2, playground +3) |
| `undra-bindgen` `typecheck_kotlin` with `GRADLE_HOME` (goldens compiled and run against the real runtime) | passed |
| TS runtime `npm test` + typecheck | 927 passed (897 + 30) |
| Kotlin `scripts/test-local.sh` | 495 cases, 0 failed, 2 skipped (454 + 41) |
| Swift `swift test` | 365 passed (328 + 37); the package builds for the iOS simulator (`xcodebuild`, no warnings) |
| `crates/undra-ffi/tests/wasm/run.sh` / `c/run.sh` / `swift/run.sh` | 29 passed / ok / passed |
| `contract-tests/run-all.sh` | ts 18/18, kotlin 18/18, swift 18/18 |
| `cargo test -p undra-bench --test budgets --release` | 2 passed |
| playground web `npm test` / `npm run build` | 64 passed / built |
| `UNDRA_TEST_IOS=1 UNDRA_TEST_ANDROID=1 cargo test -p undra-cli --test platforms` | 4 passed |
| playground Android `./gradlew :app:assembleDebug` (with `android-adapters`) | BUILD SUCCESSFUL |

## Measurement

The design's probe method on the TS mirror (Node 24.21, Apple M5 Pro, shared machine at load 8-13; the
runtime built from the commit before and after this piece; a store `_apply` shaped like the generated one;
1,667 change-sets per frame, i.e. 100 k/s at 60 Hz; p50 over 240 frames after 60 warm-up; three runs):

| Workload | Before, per frame | After, per frame | Drain alone, before → after |
|---|---|---|---|
| one-op keyed patches (`Update` of a `{ id, title, version }` row) on 10,000 rows | 3.7-4.4 ms | 0.33-0.45 ms | 3.6-4.1 ms → 0.18-0.26 ms |
| `u64` full values | 0.24-0.36 ms | 0.21-0.29 ms | 0.10-0.14 ms → 0.013-0.019 ms |

What is left per frame is mostly parsing each change-set's entry table on arrival. Kotlin and Swift were not
measured per frame here (the device phase does that, on the blueprint's devices); their backlog tests enqueue
1,000,000 change-sets in about 0.2 s (JVM) and 1.8 s (Swift debug build) and converge in one drain.

## For the integrator

* `.10x/status.md`: the matrix grows (Rust, TS, Kotlin, Swift counts below; contracts 54/54).
* The S1b stress screen can build on `Stress` (`StressMode` gains `Churn` and `Board`) and on
  `mirror.addDrainListener`; `examples/playground/web/src/embed-stats.ts` still wraps `mirror.flush` and could move
  to the listener. The landing page's "before ADR-031" numbers can be swapped (D3).
* The `undra init` Android template still uses the runtime's paced default; it could depend on
  `android-adapters` and pass `ChoreographerFramePacer` like the playground does.
* An `android-adapters` module now exists, so its README's future items (port adapters, `UndraAndroid.load`)
  have a home. Android Studio users of the playground need `ANDROID_HOME` or `sdk.dir` in a `local.properties`
  the runtime build reads (this build's own, or the opened project's).
* `typecheck_kotlin` in `undra-bindgen` silently skips on this machine: `kotlinc --version` exits 1 with Kotlin
  2.4.20, and the fallback looks in `/opt/gradle`. Run it with `GRADLE_HOME` pointing at the Gradle 8.14.3
  distribution (it passes); a follow-up could probe `kotlinc -version`.

## Verification after merging `main` (`33e4172`: ADR-032 Swift error channel, ADR-033 wire magic, the stress harness, distribution)

Merge conflicts: ADR-031 (kept the accepted version), SPEC §17.3 and Swift `LoadOptions`/`UndraCore` (both
ADR-032's `onError` / `report` / `shared` placeholder and this piece's mirror options kept), the SDE index
(main's copy carried stray `|||||||` merge markers; resolved to a clean list). Goldens and playground bindings
regenerated through their mechanisms (the generated `Stress` now has ADR-032's throwing init and reporting
command). The Kotlin runner runs S18 before S17, whose new last step (S17.6) shuts the core down.

| Check | Result |
|---|---|
| `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | 2,167 passed, 0 failed, 10 ignored |
| `undra-bindgen` `typecheck_kotlin` with `GRADLE_HOME` | passed |
| TS runtime `npm test` + typecheck | 927 passed |
| Kotlin `scripts/test-local.sh` | 495 cases, 0 failed, 2 skipped |
| Swift `swift test` | 421 passed (with ADR-032's tests) |
| ffi wasm / C / Swift acceptance | 29 passed / ok / passed |
| `contract-tests/run-all.sh` | 18 × 3: all pass (54/54) |
| bench budget gate | 2 passed |
| playground web | 64 passed, build ok |
| `UNDRA_TEST_IOS=1 UNDRA_TEST_ANDROID=1` platform tests | 4 passed |
| iOS: playground built for the iPhone 17 Pro simulator (the `CADisplayLink` path), four tabs launched and alive, no fault or Undra error in the log, XCUITest tour 5/5 | pass (screenshots outside the repo: `simctl` may not write into the worktree here, so `ios/smoke.sh`'s own screenshot step failed; its steps were run by hand) |
| Android: release core, `assembleDebug` with `android-adapters`, installed on the emulator, four tabs alive, "Stream updates" applied live through `ChoreographerFramePacer`, no `FATAL EXCEPTION` | pass |

