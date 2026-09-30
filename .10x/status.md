# Keel v1 — status

Updated: 2026-09-30 (integrator takeover, branch `claude/keel-framework-takeover-66c4ea`)

## Phase

Implementation (Phase 4 of the 10x flow). Strategy/design live in `docs/blueprint.html`
and `docs/SPEC.md`; the plan of record is `docs/HANDOFF.md`.

## Test truth (all green as of the update above)

| Suite | Result |
|---|---|
| Rust `cargo test --workspace` | 2,109 passed / 0 failed |
| TS `npm test` (runtimes/ts/@keel/runtime) | 869 passed |
| Kotlin `scripts/test-local.sh` | 454 cases, 0 failed (JNI smoke passes against libkeel_ffi) |
| Swift `swift test` (needs full Xcode; env.sh sets DEVELOPER_DIR) | 328 passed / 0 failed |

## Done

- Merged the four outstanding branches into the takeover line: `wt/integrate`,
  `wt/ts-core`, `wt/kotlin-core`, `wt/swift-core`.
- First native Swift compile: one `@unchecked Sendable` restatement fixed; suite green.
- Kotlin stream-cancellation test de-raced (collector now parks so cancel lands mid-stream).
- Toolchain installed on this Mac: rustup 1.98.1 + ios/android/wasm targets, JDK 17,
  Kotlin 2.4.20, Gradle, binaryen; kotlinx-coroutines jar in `../.tools/lib`.
- `scripts/env.sh` finds the native macOS toolchain (rustup, brew, Xcode DEVELOPER_DIR).
- `.10x/` state recreated (the original was never committed); `docs/HANDOFF.md` rewritten
  as the plan of record.

## In progress


## Landed since takeover

- **fast-dispatch merged** (ADR-028): a per-thread reply slot (no unsafe, no ABI change)
  makes call_sync allocation-free on the hot path — 73.8 -> 43.9 ns (31.5 ns via the new
  call_sync_with), C ABI 79.3 -> 49.8 ns; replies byte-identical under a counting
  allocator across every outcome incl. layers and panics. Both blueprint §14 host rows
  that missed now measure within target; device verdicts land with the device phase.

- **cli-polish merged**: @rpath install name, platform-scoped toolchain notes, jniLibs
  drift detection (the silent no-core APK now warns with the line to fix), debug-size
  hint, kotlin .gitignore emitted by bindgen output, workspace target-dir reuse (no more
  second 1.4 GB tree), and a real `keel dev` watcher race fixed (10/10 under load).
- **bindgen Swift naming fixed**: a store signal named a reserved word (`default`) spells
  as `default_` — @Observable rejects backticked stored properties. Goldens refreshed.
- **CI**: playground-core added to the wasm32 loop; contract runners (ts+kotlin on Linux,
  swift on macOS) are jobs.

- **keyed-ops merged** (ADR-027): recorded list operations make keyed change-sets
  O(change) — 10k insert 536us -> 6.3us, update 272ns; model-based proptest at 100k cases;
  blueprint row moved from MISS to within. Playground BigList + bench hook now ride the
  recorded path. Raw update()/set() keep the diff fallback.
- **query-rollback merged**: a failed mutation's rollback is the inverse of its OWN writes
  (per-entry write stamps + optimistic layers with hand-down); 8 regressions fail on the
  old code. SPEC §9 made precise.
- **platform-polish merged**: Swift generated streams apply real §3.7 backpressure;
  InprocTransport checks the schema hash before keel_init; TS Mirror drains subscriber
  enqueues; Sendable restated in generated Swift; @keel/runtime gains react/vue/svelte/
  solid adapters (SPEC §10.3) and the playground web app uses the react one.
  Post-merge sweep: Rust 2,047 · TS 869 · Swift 328 · contracts 51/51, clippy clean.
  Known small issue (pre-existing): the `stores` bindgen golden has a signal named
  `default`; Swift @Observable rejects the backticked property — needs a rename rule.

- **playground merged** — the end-to-end proof. One core (todos, counter, 10k keyed list,
  remote query/mutations, lab, bench hooks; 47 tests), React/SwiftUI/Compose apps RUN on
  Chrome (48 ms interactive, 62 fps under 10k-list updates), the iPhone 17 Pro simulator
  (XCUITests 5/5) and the `keel` AVD, with proof screenshots in examples/playground/.proof.
  **All 51 contract cells pass (S01-S17 × 3 platforms), re-verified on the merged tree.**
  Found 8 defect groups (recorded in .10x/decisions/sde/playground.md); fixes in flight.
  Playground bindings regenerated post-macros; env.sh no longer prefers a non-executable
  portable toolchain.

- **bench harness merged**: 45 gated ops sharing one workload source with the criterion
  benches; budgets.toml at 5x quiet-host medians; bench.yml CI gate; RESULTS.md v1 table.
  Honest misses recorded and now in flight as ADR'd fixes (keyed diff, sync-call allocs).
  Also surfaced: dropping a Runtime without shutdown() keeps its threads up to ~5 s (the
  query hydrate task's bounded Kv retry holds a Ctx). All shipped embedders call
  shutdown(); WeakCtx-based task handles are v1.x debt — document in Runtime docs after
  the dispatch piece merges.

- **keel-ffi fix round merged** (ADR-026): port-registration refcounting drains in-flight
  callbacks before unregister/shutdown returns (H1 UAF, ASan-verified both ways);
  out_reply is always C-allocator-owned (M1, Miri-clean); keel.h now carries the full
  host contract incl. the corrected callable-from-callback list; C harness and fresh-dist
  wasm legs revived and in CI; Log-port reply loop fixed; wasm alloc/reply hardening;
  runtime Subscription leak fixed (heap flat over 1000 cycles). TS 833. Re-review pending.
  Local-run note: the C harness rebuilds keel-ffi without `jni`, clobbering the dylib —
  build `--features jni` immediately before Kotlin runs (CI jobs are isolated).
  Re-review found N1 (a NEW UAF in the fix: the loser of a removal race did not wait) and
  N2 (mutual-removal hang); the integrator fixed both (shared draining list; in-callback
  removals assert/skip) and the reviewer confirmed closure under ASan. keel-ffi cycle
  CLOSED. All four core-crate adversarial cycles (signals, runtime, macros, ffi) are
  now complete with every High/Medium finding fixed and re-verified.

- **keel-macros fix round merged** (ADR-025): compile-time schema-identity checks
  (E0060/E0061), typed port outcomes via From<PortError> (E0033; HttpError/FsError map
  Unavailable etc. — wire unchanged), store error-recovery without cascades, seven new
  E-codes, diagnostics polish. BREAKING for users: a port `Result` method's error type now
  needs `From<PortError>`. Re-review CLOSED (sound for v1 on schema/wire); v1.x polish:
  query-in-impl diagnostics, split-impl follow-ons, two Low wording nits.
- **keel-runtime re-review**: all original findings CLOSED; new NF1 (hostile snapshot
  floor exhausts process-wide generations permanently) fixed by the integrator with a
  2^24-headroom ceiling + tests; NF2 (eviction WARN flood) rate-limited to 1/1024.
  v1.x items: L3 release-build silent drop of off-runtime writes; L2 init-hook timing.

- **bindgen stdlib filter merged** (ADR-024): per-app bindings no longer duplicate the
  standard ports/records — references point at each runtime's own types (exact-shape
  matching; E0052 for a name collision with a foreign id). Template bindings 2,836→942
  lines. New `stdlib` golden compiled+run for TS and Kotlin. Open (recorded in ADR-024):
  Swift runtime could make its Port* types public to drop the reachable-only fallback.
- **CI workflows added** (.github/workflows/ci.yml): Linux rust gates, TS, Kotlin+JNI,
  wasm acceptance, macOS Swift + C ABI + iOS cross-checks, Android cargo-ndk, Miri+ASan.

- **keel-runtime fix round merged** (ADR-022, ADR-023): global generation counter +
  `generation_floor` in the Snapshot payload (all three platform codecs updated); observe
  and restore deliver under the store's delivery lock via `StoreCell::observe_and_deliver`;
  E_REENTRANT now also fires for host-callback re-entry (was a deadlock); restore cancels
  calls whose receiver was replaced; shutdown answers all in-flight work before joining;
  write checker is an allowlist and TestRuntime uses a real blocking pool. Full matrix
  re-verified. Detail: .10x/decisions/sde/keel-runtime-review-fixes.md. Re-review pending.

- **keel-cli** merged after review: init/bindgen/build/dev/doctor/adopt; XCFramework,
  16KB-aligned Android .so, wasm-opt'd wasm; teaching C00NN errors. The generated shells
  RAN on the iOS 26.5 simulator, an Android emulator and Chrome against the real core.
  Sizes: wasm 85 KB gz (budget 120), Android 831 KB (budget 1.2 MB). Also fixed a real
  browser bug in the TS mirror (queueMicrotask receiver → change-sets dropped; TS 831).
  Deviation accepted: dlopen unsafe in cli's schema.rs (SPEC §13) — CLAUDE.md R2 amended.
  Open: keel_schema_json is canonical (docs dropped) → --docs uses the runner; Swift
  Package stand-in core needs KEEL_LINK_CORE=1 under Xcode-from-Dock; no Android remote
  mode in the Kotlin runtime; dev clients don't auto-reconnect.
- **keel-signals fix round closed**: all 8 findings fixed + re-reviewed (original reviewer,
  own repros); residual R2 fixed by the integrator (observe txn across delivery), R1/R3
  documented. keel-signals is the most battle-tested crate in the repo.
- **Facade/schema decision**: every core linking keel-ports carries the standard ports in
  its schema (R1); bindgen will filter std definitions from per-app generated code
  (ADR-024, in flight) because the runtimes hand-implement exactly those types (R3).
  Interim: cli's embedded template schema regenerated; everything green.

- **keel-query** (SPEC §9, ADR-018) merged after review: QueryClient (staleness, dedup,
  retry w/ Rng jitter, gc), QueryHandle serving the bindgen-golden wire shape (ids
  0x54209c7c/0x21d1b9e2/0x44cec2fa/0x4abb0ec8 locked), optimistic mutations with
  single-transaction rollback, offline queue (schema-hash-guarded), debounced Kv
  persistence + hydration. Runtime additions per ADR-018: DispatchLayer fall-through
  (zero cost on static hits), transient objects skip snapshots; macros emit
  Query/MutationRegistration. Facade: keel::query is a shim; ports re-exported;
  CtxPorts + CtxQuery in the prelude. Cross-branch interaction fixed in integration:
  transport's silent_for now exempts dev chatter (Log, hydration PortCall).
  Known debt: persisted cache entries never gc'd from Kv; queued mutations lose their
  .invalidates list across restart; interval_ms not in the v1 contract (no carrier).

- **keel-transport** merged after review: WebSocket server for `keel dev` (Bridge Host,
  never blocks the core; seq assigned under the queue lock; overflow aborts the lagging
  client; origin policy LocalNetwork; keepalive; release-on-disconnect for dev relaunch).
  124 real-socket tests + byte fuzz; interop-verified against the unmodified TS, Kotlin
  and Swift clients. p50 sync call over loopback ~21 µs. Workspace: 1,400.
  Note for cli: dev cores must bind Rust Clock/Rng/Log (sync ports can't be remote);
  `keel::dev::serve()` facade wiring is an integrator follow-up.
- **Adversarial review: keel-signals** (.10x/reviews/2026-09-30-keel-signals-review.md):
  keyed diff CONFIRMED correct (80k property cases, all three platforms' Move semantics);
  1 High (panic mid-commit → silent host divergence), 3 Medium (observe ordering,
  cross-thread writes, effect-cap stranding), 4 Low. Fix round in flight.

- **keel-ffi** (SPEC §6/§6.1/§7) merged after adversarial review: C ABI (19 fns, panic
  guard at every entry, SAFETY lint-enforced), JNI shim (RegisterNatives, direct buffers,
  daemon-attached callback threads), wasm exports/imports verified against the real TS
  runtime (10/10) and hand-written host (16/16); Kotlin NativeSmokeTests pass against the
  real dylib (454 cases 0 failed). Crossing bench: ~81 ns keel_call_sync. Workspace: 1,276.
  Follow-ups for keel-cli: name the cdylib `keel_core` (or pass keel.native.name);
  XCFramework static linking needs -force_load in debug (release LTO links clean).
  Known issue: macro-generated port proxies panic when a port is unavailable → traps on
  wasm (native contains it as status 2); document adapters as required on web, or teach
  the proxies a typed fallback in a later pass. SPEC §6/§6.3/§7 updated to match shipped
  reality (init/restore codes, out_reply ownership, log routing, core_threads=0→1).

- **keel-ports** (SPEC §8) merged after adversarial review: ten ports, records with
  byte-golden layout locks, id parity vs the Kotlin constants, deterministic fakes
  (FakeClock with deadline-time reads), `fakes::install(&TestRuntime)`. Workspace: 1,235.
  Debt noted: no bench yet (bench/ is a stub; lands with the bench piece); FakeHttp has
  no hold/release gate for in-flight cancellation tests.

## Remaining (ordered, see docs/HANDOFF.md §2)

keel-cli → playground (3 apps) → contract scenarios on 3 platforms → bench/RESULTS.md +
CI → adversarial reviews closed.

## Environment notes

- Shrey ran `xcode-select -s` to full Xcode 26.6; iOS 26.5 simulators installed.
- Android SDK at /opt/homebrew/share/android-commandlinetools (platform-tools, android-35,
  build-tools 35, NDK 27.2.12479018, emulator, arm64 system image, AVD `keel`); cargo-ndk
  installed. `sudo` remains unavailable to the agent.

## Playground and contract tests (branch `wt/playground`, 2026-09-30)

- **Playground landed**: `examples/playground/core` (47 tests; todos, counter, 10k keyed list, remote query +
  optimistic/offline commands, lab, bench hooks), generated bindings, React/Vite, SwiftUI and Compose apps that
  ran on headless Chromium, the iPhone 17 Pro simulator and the `keel` AVD (proof in `examples/playground/.proof`).
- **Contract scenarios S01..S17 pass on all three platforms** (TS over wasm, Kotlin over JNI, Swift over the C
  ABI): `contract-tests/run-all.sh`. Workspace `cargo test`: 1,883 passed, 0 failed, 7 ignored (1,836 + 47).
- **Findings for the integrator** (details in `decisions/sde/playground.md`): keyed patch cost is O(list)
  (471 us native vs a 20 us budget), keel-query rollback drops a later placeholder, generated Swift streams lose
  backpressure, Swift `load` inits before the schema check, TS Mirror strands a subscriber-enqueued change-set.
