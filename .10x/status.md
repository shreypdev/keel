# Undra v1 — status

> **Note (2026-09-30, evening):** the product was renamed Keel → Undra (ADR-030) by a mechanical
> script; identifiers in the entries below were rewritten with it, so branch names and crate names
> in older entries read `undra…` here while git history still says `keel…`.

Updated: 2026-09-30 (integrator takeover, branch `claude/undra-framework-takeover-66c4ea`)

## Phase

**v1 COMPLETE** (2026-09-30). Every item of docs/HANDOFF.md §5 is met; the final
verification matrix (fmt, clippy, Rust 2,110, TS 897, Kotlin 454, Swift 328, wasm 29,
C harness, budgets gate in release, contracts 51/51) ran green on this tree in one pass.

Honest caveats: (1) the bench rows that name devices (A15 / mid-range Android / Chromium
budgets, hello-world sizes) carry host-proxy numbers within budget; device-measured
verdicts land with a device phase. (2) CI workflows are authored and YAML-validated but
have not executed on GitHub runners from this machine. (3) The v1.x debt list lives in
the "Landed" notes below and .10x/reviews resolutions.

## Test truth (all green as of the update above)

| Suite | Result |
|---|---|
| Rust `cargo test --workspace` | 2,109 passed / 0 failed |
| TS `npm test` (runtimes/ts/@undra/runtime) | 897 passed |
| Kotlin `scripts/test-local.sh` | 454 cases, 0 failed (JNI smoke passes against libundra_ffi) |
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

(nothing — v1 is complete)


## Landed since takeover

- **cdylib schema/JNI dead-strip fixed** (ADR-029, `wt/schema-strip`): on a clean macOS dev
  build `undra build --platform host` produced a `libundra_core.dylib` that reported the empty
  schema `0x98754cbea76a32b2` and exported no JNI symbols — the app core's `inventory`
  registrations and undra-ffi's `#[no_mangle]` JNI exports (both in dependency rlibs, linked with
  rustc's `--start-lib` lazy semantics) were dead-stripped, because incremental compilation (the
  dev default, and worst of all a polluting incremental core rlib left by a plain `cargo build`) is
  the trigger. `undra bindgen`'s dlopen path read empty and the Kotlin contract column failed
  (`UnsatisfiedLinkError` / S16 schema mismatch); Swift hid it (its test binary references undra
  symbols), ELF (Linux/Android `.so`) is unaffected, release and iOS (`-force_load`) hid it because
  they are non-incremental. Fix: the shim's `[profile.dev]` is `incremental = false`, and `undra
  build` compiles the **host** library in a target directory of its own
  (`<target>/undra/<project>/host-lib`) with `CARGO_INCREMENTAL=0` — so it never reuses a
  stripping-prone incremental rlib a plain `cargo build`/`cargo test` left in the shared target
  (decision 6). Only the host library needs the private dir (Android is ELF, iOS is `-force_load`ed).
  Verified robust across the clean / `cargo build` / `cargo test --workspace` / inherited-
  `CARGO_INCREMENTAL=1` matrix (fat LTO, `codegen-units=1`, and `incremental=false` alone each failed
  part of it). Clean verification: host cdylib reads `0x0f95cc4a…` + JNI present, `undra bindgen
  --docs --check` green, contract-tests 51/51 on all three columns, wasm + workspace (2,109) green;
  host build ~5 s clean / <1 s cached. Regression: `crates/undra-cli/tests/schema_retention.rs`
  (gated, in CI on Linux + macOS). The pre-existing v1.x item "`undra_schema_json` full-JSON variant"
  is unrelated (docs in the dlopen path), and the flaky load-sensitive
  `undra-transport::lifecycle::a_chatty_client_is_never_pinged` is unchanged by this work.

- **fast-dispatch merged** (ADR-028): a per-thread reply slot (no unsafe, no ABI change)
  makes call_sync allocation-free on the hot path — 73.8 -> 43.9 ns (31.5 ns via the new
  call_sync_with), C ABI 79.3 -> 49.8 ns; replies byte-identical under a counting
  allocator across every outcome incl. layers and panics. Both blueprint §14 host rows
  that missed now measure within target; device verdicts land with the device phase.

- **cli-polish merged**: @rpath install name, platform-scoped toolchain notes, jniLibs
  drift detection (the silent no-core APK now warns with the line to fix), debug-size
  hint, kotlin .gitignore emitted by bindgen output, workspace target-dir reuse (no more
  second 1.4 GB tree), and a real `undra dev` watcher race fixed (10/10 under load).
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
  InprocTransport checks the schema hash before undra_init; TS Mirror drains subscriber
  enqueues; Sendable restated in generated Swift; @undra/runtime gains react/vue/svelte/
  solid adapters (SPEC §10.3) and the playground web app uses the react one.
  Post-merge sweep: Rust 2,047 · TS 869 · Swift 328 · contracts 51/51, clippy clean.
  Known small issue (pre-existing): the `stores` bindgen golden has a signal named
  `default`; Swift @Observable rejects the backticked property — needs a rename rule.

- **playground merged** — the end-to-end proof. One core (todos, counter, 10k keyed list,
  remote query/mutations, lab, bench hooks; 47 tests), React/SwiftUI/Compose apps RUN on
  Chrome (48 ms interactive, 62 fps under 10k-list updates), the iPhone 17 Pro simulator
  (XCUITests 5/5) and the `undra` AVD, with proof screenshots in examples/playground/.proof.
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

- **undra-ffi fix round merged** (ADR-026): port-registration refcounting drains in-flight
  callbacks before unregister/shutdown returns (H1 UAF, ASan-verified both ways);
  out_reply is always C-allocator-owned (M1, Miri-clean); undra.h now carries the full
  host contract incl. the corrected callable-from-callback list; C harness and fresh-dist
  wasm legs revived and in CI; Log-port reply loop fixed; wasm alloc/reply hardening;
  runtime Subscription leak fixed (heap flat over 1000 cycles). TS 833. Re-review pending.
  Local-run note: the C harness rebuilds undra-ffi without `jni`, clobbering the dylib —
  build `--features jni` immediately before Kotlin runs (CI jobs are isolated).
  Re-review found N1 (a NEW UAF in the fix: the loser of a removal race did not wait) and
  N2 (mutual-removal hang); the integrator fixed both (shared draining list; in-callback
  removals assert/skip) and the reviewer confirmed closure under ASan. undra-ffi cycle
  CLOSED. All four core-crate adversarial cycles (signals, runtime, macros, ffi) are
  now complete with every High/Medium finding fixed and re-verified.

- **undra-macros fix round merged** (ADR-025): compile-time schema-identity checks
  (E0060/E0061), typed port outcomes via From<PortError> (E0033; HttpError/FsError map
  Unavailable etc. — wire unchanged), store error-recovery without cascades, seven new
  E-codes, diagnostics polish. BREAKING for users: a port `Result` method's error type now
  needs `From<PortError>`. Re-review CLOSED (sound for v1 on schema/wire); v1.x polish:
  query-in-impl diagnostics, split-impl follow-ons, two Low wording nits.
- **undra-runtime re-review**: all original findings CLOSED; new NF1 (hostile snapshot
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

- **undra-runtime fix round merged** (ADR-022, ADR-023): global generation counter +
  `generation_floor` in the Snapshot payload (all three platform codecs updated); observe
  and restore deliver under the store's delivery lock via `StoreCell::observe_and_deliver`;
  E_REENTRANT now also fires for host-callback re-entry (was a deadlock); restore cancels
  calls whose receiver was replaced; shutdown answers all in-flight work before joining;
  write checker is an allowlist and TestRuntime uses a real blocking pool. Full matrix
  re-verified. Detail: .10x/decisions/sde/undra-runtime-review-fixes.md. Re-review pending.

- **undra-cli** merged after review: init/bindgen/build/dev/doctor/adopt; XCFramework,
  16KB-aligned Android .so, wasm-opt'd wasm; teaching C00NN errors. The generated shells
  RAN on the iOS 26.5 simulator, an Android emulator and Chrome against the real core.
  Sizes: wasm 85 KB gz (budget 120), Android 831 KB (budget 1.2 MB). Also fixed a real
  browser bug in the TS mirror (queueMicrotask receiver → change-sets dropped; TS 831).
  Deviation accepted: dlopen unsafe in cli's schema.rs (SPEC §13) — CLAUDE.md R2 amended.
  Open: undra_schema_json is canonical (docs dropped) → --docs uses the runner; Swift
  Package stand-in core needs UNDRA_LINK_CORE=1 under Xcode-from-Dock; no Android remote
  mode in the Kotlin runtime; dev clients don't auto-reconnect.
- **undra-signals fix round closed**: all 8 findings fixed + re-reviewed (original reviewer,
  own repros); residual R2 fixed by the integrator (observe txn across delivery), R1/R3
  documented. undra-signals is the most battle-tested crate in the repo.
- **Facade/schema decision**: every core linking undra-ports carries the standard ports in
  its schema (R1); bindgen will filter std definitions from per-app generated code
  (ADR-024, in flight) because the runtimes hand-implement exactly those types (R3).
  Interim: cli's embedded template schema regenerated; everything green.

- **undra-query** (SPEC §9, ADR-018) merged after review: QueryClient (staleness, dedup,
  retry w/ Rng jitter, gc), QueryHandle serving the bindgen-golden wire shape (ids
  0x54209c7c/0x21d1b9e2/0x44cec2fa/0x4abb0ec8 locked), optimistic mutations with
  single-transaction rollback, offline queue (schema-hash-guarded), debounced Kv
  persistence + hydration. Runtime additions per ADR-018: DispatchLayer fall-through
  (zero cost on static hits), transient objects skip snapshots; macros emit
  Query/MutationRegistration. Facade: undra::query is a shim; ports re-exported;
  CtxPorts + CtxQuery in the prelude. Cross-branch interaction fixed in integration:
  transport's silent_for now exempts dev chatter (Log, hydration PortCall).
  Known debt: persisted cache entries never gc'd from Kv; queued mutations lose their
  .invalidates list across restart; interval_ms not in the v1 contract (no carrier).

- **undra-transport** merged after review: WebSocket server for `undra dev` (Bridge Host,
  never blocks the core; seq assigned under the queue lock; overflow aborts the lagging
  client; origin policy LocalNetwork; keepalive; release-on-disconnect for dev relaunch).
  124 real-socket tests + byte fuzz; interop-verified against the unmodified TS, Kotlin
  and Swift clients. p50 sync call over loopback ~21 µs. Workspace: 1,400.
  Note for cli: dev cores must bind Rust Clock/Rng/Log (sync ports can't be remote);
  `undra::dev::serve()` facade wiring is an integrator follow-up.
- **Adversarial review: undra-signals** (.10x/reviews/2026-09-30-undra-signals-review.md):
  keyed diff CONFIRMED correct (80k property cases, all three platforms' Move semantics);
  1 High (panic mid-commit → silent host divergence), 3 Medium (observe ordering,
  cross-thread writes, effect-cap stranding), 4 Low. Fix round in flight.

- **undra-ffi** (SPEC §6/§6.1/§7) merged after adversarial review: C ABI (19 fns, panic
  guard at every entry, SAFETY lint-enforced), JNI shim (RegisterNatives, direct buffers,
  daemon-attached callback threads), wasm exports/imports verified against the real TS
  runtime (10/10) and hand-written host (16/16); Kotlin NativeSmokeTests pass against the
  real dylib (454 cases 0 failed). Crossing bench: ~81 ns undra_call_sync. Workspace: 1,276.
  Follow-ups for undra-cli: name the cdylib `undra_core` (or pass undra.native.name);
  XCFramework static linking needs -force_load in debug (release LTO links clean).
  Known issue: macro-generated port proxies panic when a port is unavailable → traps on
  wasm (native contains it as status 2); document adapters as required on web, or teach
  the proxies a typed fallback in a later pass. SPEC §6/§6.3/§7 updated to match shipped
  reality (init/restore codes, out_reply ownership, log routing, core_threads=0→1).

- **undra-ports** (SPEC §8) merged after adversarial review: ten ports, records with
  byte-golden layout locks, id parity vs the Kotlin constants, deterministic fakes
  (FakeClock with deadline-time reads), `fakes::install(&TestRuntime)`. Workspace: 1,235.
  Debt noted: no bench yet (bench/ is a stub; lands with the bench piece); FakeHttp has
  no hold/release gate for in-flight cancellation tests.

## Remaining (ordered, see docs/HANDOFF.md §2)

undra-cli → playground (3 apps) → contract scenarios on 3 platforms → bench/RESULTS.md +
CI → adversarial reviews closed.

## Environment notes

- Shrey ran `xcode-select -s` to full Xcode 26.6; iOS 26.5 simulators installed.
- Android SDK at /opt/homebrew/share/android-commandlinetools (platform-tools, android-35,
  build-tools 35, NDK 27.2.12479018, emulator, arm64 system image, AVD `undra`); cargo-ndk
  installed. `sudo` remains unavailable to the agent.

## Playground and contract tests (branch `wt/playground`, 2026-09-30)

- **Playground landed**: `examples/playground/core` (47 tests; todos, counter, 10k keyed list, remote query +
  optimistic/offline commands, lab, bench hooks), generated bindings, React/Vite, SwiftUI and Compose apps that
  ran on headless Chromium, the iPhone 17 Pro simulator and the `undra` AVD (proof in `examples/playground/.proof`).
- **Contract scenarios S01..S17 pass on all three platforms** (TS over wasm, Kotlin over JNI, Swift over the C
  ABI): `contract-tests/run-all.sh`. Workspace `cargo test`: 1,883 passed, 0 failed, 7 ignored (1,836 + 47).
- **Findings for the integrator** (details in `decisions/sde/playground.md`): keyed patch cost is O(list)
  (471 us native vs a 20 us budget), undra-query rollback drops a later placeholder, generated Swift streams lose
  backpressure, Swift `load` inits before the schema check, TS Mirror strands a subscriber-enqueued change-set.

## Launch v2 (2026-09-30, evening) — integrator ledger

Spec: `.10x/specs/2026-09-30-launch-v2-design.md` (+ Amendment A). Decisions under
`.10x/decisions/{cto,product-manager,architect,devops,qa}/launch-v2.md`.

| Piece | Merge | Verdict |
|---|---|---|
| cdylib dead-strip fix (ADR-029) | `1a4b038` | contracts green on CI for the first time; Rust 2,110 |
| contract runner diagnostics, S04 spacing, fixed-finding test, per-call alloc assertion | `e2374f4`…`dd5a2cd` | CI all-green twice (`1b550c1`, `dd5a2cd`) — first fully green runs of the project |
| state: spec, ADR-030 (Undra), role decisions | `2d4f0fe`, `6a0a67a` | — |
| rename Keel → Undra (ADR-030; 1,290 files; `scripts/rename-keel-to-undra.sh`) | `31a1f37` | every suite green on the branch incl. iOS sim + Android emulator launches; repo renamed to `shreypdev/undra` |
| docs truth pass (blueprint claims, README label/count, TS count) | `4fdd36e`, `0b0f292` | from the blog fact-check |
| site v2 (v1 palette, 332-word landing, live demo, roadmap, SEO, error codes page) + 4 fact-checked posts | `c9e6bf8` | fable reviews: `.10x/reviews/2026-09-30-site-v2-review.md`, `…-blog-factcheck.md` |

In flight (worktrees under `/Users/shrey/Desktop/src/.work/`): `stress` (harsh-conditions harness,
S1a), `dist` (release pipeline, npm/brew/curl), `magic` (ADR-033 envelope magic `UNDR`),
`coalesce` (ADR-031 frame-coalesced delivery), `swift-errors` (ADR-032 Swift error channel).
Matrix at checkpoint 1: Rust 2,110 · TS 897 · Kotlin 454 · Swift 328 · wasm 29 · contracts 51/51.
Matrix at checkpoint 2: Rust 2,156 · TS 897 · Kotlin 454 · Swift 384 · wasm 29 · contracts 51/51 (S18 lands with ADR-031).

### Checkpoint 2 (2026-10-01, early morning)

| Piece | Merge | Verdict |
|---|---|---|
| ADR-033 envelope magic `UNDR` + `sync-vectors --check` in CI | `88082b8`, `7b10e0e` | all suites; CI/Site/Bench green |
| distribution: `release.yml`, npm packages (`@undra/cli*`, `undra`), Homebrew template, `site/install.sh`, `bump-version.sh`, `docs/RELEASING.md`, `undra --version`, `undra init` pins by tag | `11feefd`, `2f3e1c6` | fable security review `.10x/reviews/2026-09-30-dist-review.md` (0 High, 2 Medium fixed); **release dry run green end to end** (verify, 4 builds, package); first dry run caught a pipefail false negative in the tarball check |
| harsh-conditions benchmark harness (S1a) + 7 landing-page cards | `2e0d1c5`, `74ddb44` | opus review `.10x/reviews/2026-09-30-stress-bench-review.md` (H1 churn invariant fixed); the `[stress]` gates and the 10 s soak **pass on the GitHub runner** (Bench job) |
| ADR-032 Swift error channel (generated Swift never traps) | `1e50f96` | opus review `.10x/reviews/2026-09-30-swift-error-channel-review.md`: R6 holds for Swift; Rust 2,156 · Swift 384 · contracts 51/51 |

Review follow-ups recorded, not yet done: stress M1 (gates would not catch a 2× regression —
ratio gates or runner baselines), M3 (a contended-writes scenario), M4 (a real drift gate), L4–L8;
dist: the `release` environment + `v*` tag ruleset are founder steps.

### Checkpoint 3 (2026-10-01, morning)

| Piece | Merge | Verdict |
|---|---|---|
| transport tests deterministic (injected clock; the fuzz test sized to the flood guard; TIME_WAIT fix) | `33e4172` | 100/100 and 200/200 loaded runs |
| `UNDRA_BENCH_SCALE=2` in `bench.yml` | `b993067` | the slowest runner class is 6.2× the host on `changeset_100/cell`; Bench green since |
| **ADR-031 frame-coalesced delivery** — all three runtimes, `no_coalesce` through the schema, `Stress` store, S18 | `ce4fd85` | opus review `.10x/reviews/2026-10-01-frame-coalesced-delivery-review.md`: mirror-equals-core holds (model tests 30k/20k/20k histories); 2 Medium fixed (either patch bound drops; Kotlin drain no longer chases); Rust 2,168 · TS 931 · Kotlin 500 · Swift 425 · **contracts 54/54** |

Every piece of the launch-v2 spec (§7) is merged. In flight: `bench-followups` (stress review M1/M3/M4/L4–L8)
and `stress-screen` (S1b: the playground stress screen and the landing page's "Push it").
Matrix at checkpoint 3: Rust 2,168 · TS 931 · Kotlin 500 · Swift 425 · wasm 29 · contracts 54/54 (18 scenarios × 3).

### Checkpoint 4 (2026-10-01, late morning) — launch v2 complete

| Piece | Merge | Verdict |
|---|---|---|
| S1b playground stress screen + landing "Push it" | `a4efd8d` | fable review `.10x/reviews/2026-10-01-stress-screen-review.md`: live numbers honest; 100k updates/s → 120 applies/s after merge, drain p99 ≈ 1 ms, 0 dropped frames; 2,185 Rust · 102 web |
| roadmap data: shipped since v1.0 | `7c1a0b5` | — |
| port tests wait for the Echo port's call (dev-mode Kv/Wall calls arrived first on a slow runner) | `74106e1` | 30 loaded runs clean; CI green |
| bench follow-ups: ratio gates, same-VM base baseline in CI, contended completions, Theil–Sen drift gate, L4–L8, the completions deadline fix | `0c88c8a` | opus review `.10x/reviews/2026-10-01-bench-followups-review.md` (2 High fixed: an out-of-sample ratio bound; `cancel-in-progress` had been cancelling merge runs); first runner Bench run green — the baseline step engaged: 55 rows recorded from the base commit `74106e1` on the same VM and the head gated against them (a `vs base` column in the budgets table); 2,236 Rust |

Every piece of the launch-v2 spec and every review follow-up is merged; no worktrees remain.
Matrix at checkpoint 4: Rust 2,236 · TS 931 · Kotlin 500 · Swift 425 · web playground 102 · wasm 29 · contracts 54/54.
CI has been green on every `main` run since `1b550c1` except two flakes, both fixed (the alloc test, the port test).

## v1.x program (2026-10-01, afternoon) — integrator ledger

Spec `.10x/specs/2026-10-01-v1x-default-choice-design.md` (Tracks A–H, Amendments A–D). Phase 1 pieces
merged in dependency order after an adversarial review each; every merge ran the full local matrix first.

### Checkpoint 5 (2026-10-01, afternoon) — phase 1 landing

| Piece | Merge | Verdict |
|---|---|---|
| gap audit + ADR-034…037 drafts; competitive limitations (68, 36-row matrix); ADR-039 derived lists | `c40e2d6`, `15c6098` | design artefacts only; Amendments B–D record the decisions |
| C1+C2 schema-json: `undra_schema_json` is the whole schema (ADR-050), Swift `Port*` types public, bindgen fallback gone | `249ee26` | opus review `.10x/reviews/2026-10-01-schema-json-review.md`: a doc-only change moves the export, not the hash (probed on four paths); M1 → ADR-050 |
| E1 device-bench: `undra bench --device`, simulator/emulator/Chromium rows, `bench/results/device/*.json`, floor gates on the site | `4b2515a` | opus review `.10x/reviews/2026-10-01-device-bench-review.md`: rows publishable as labelled; the web size claim was false (85 → **135 KB gzipped, over budget**), corrected everywhere in `535f2c8`, E5 tracks the fix |
| D1 diagnostics: 44 E-codes with what/why/fix/link, an emitter and a golden each; `site/docs/errors.html` regenerates | `42fdd42` | opus review `.10x/reviews/2026-10-01-diagnostics-review.md`: R8 holds on every reachable case; 3 fixed in `1931f81`; Rust 2,296 |
| B1+B2 dev-loop: Android remote mode (RFC 6455 client in Kotlin), auto-reconnect with backoff + session resume on all three platforms (ADR-051), visible status, hostile-server tests | `a0d638f` (fast-forward after a cross-merge on the branch) | opus review `.10x/reviews/2026-10-01-dev-loop-review.md`: session token is URL material only, resume grace off by default; remote-mode smoke on the `undra` emulator; Rust 2,327 · Kotlin 555 · Swift 465 · TS 966 · contracts 54/54 |

Matrix at checkpoint 5: Rust 2,327 · TS 966 · Kotlin 555 · Swift 465 · contracts 54/54 (18 × 3).

In flight (one worktree each): `parity` (C3/C4, reviewed, crossing main) → `android-adapters`
(reviewed, awaiting cross-merge) → `runtime-lifecycle` (Track A, ADR-034/035/036 + ADR-019
amendment, implemented, opus review running) → `react-native` (G1, implemented, review running);
`tooling` (D2–D5) and `wasm-size` (E5, ADR-052) started in parallel. After those: wave 0 of the
boundary plan (`abi-table` ADR-044, `ios-floor` ADR-045, `newtypes` ADR-042), `persistence-v2`
(ADR-037/049), `dev-reload` (B3), `testkit` (F), derived lists (ADR-039), then the v1.2 bets.

### Checkpoint 6 (2026-10-01, late afternoon) — parity landed; Rust 1.99.0 handled

| Piece | Merge | Verdict |
|---|---|---|
| Rust 1.99.0 reached stable and failed every CI job on `a0d638f` (`fetch_update` deprecated, const-eval panic wording changed under the trybuild goldens) | `d59b486`, `43348eb` | `cas_update` (MSRV-safe CAS loop) replaces the three call sites; CI pins `1.98.1` in ci/bench/site/release.yml; the deliberate bump is an open item (header comment in `ci.yml` says how) |
| the schema-docs test (CI-only, `--ignored`) pinned the playground hash by hand and went stale when device-bench moved it | `3a2ff1f` | the test reads the hash from the committed bindings |
| C3+C4 parity: Kotlin/TS `UndraCallError` closed sets, `UndraTransportException` under `UndraException`, non-throwing commands with `onError`/`report`, `onError` silent for a drop the connection state reports, TS snapshot/restore, worker-mode fix, recursive records, `docs/ERRORS.md` (replaces `SWIFT_ERRORS.md`), interop scripts assert the typed failure | `143b72a` | opus review (parity) + the cross-merge record in `.10x/decisions/sde/parity.md`; Rust 2,349 · Kotlin 585 · TS 1,049 · Swift 467 · wasm 24 · contracts 54/54 |

Matrix at checkpoint 6: Rust 2,349 · TS 1,049 · Kotlin 585 · Swift 467 · wasm 24 · contracts 54/54.
In flight: `android-adapters` (crossing main), `runtime-lifecycle` (opus review), `react-native`
(cross-merge + CI jobs + Android reload re-check), `tooling` (D2–D5), `wasm-size` (E5, ADR-052),
`site-errors` (API pages for the Kotlin/TS error channel).

### Checkpoint 7 (2026-10-01, evening) — React Native landed

| Piece | Merge | Verdict |
|---|---|---|
| site: the Kotlin/TS API pages describe the typed error channel (samples compiled under kotlinc and tsc --strict); roadmap refreshed to what shipped / in flight / next | `c538222`, `afa4bfd` | docs; 342/350 landing words |
| a Kotlin test lambda CI's kotlinc 2.0.21 could not infer (brew's 2.4.20 could) | `7c8d2d8` | the Kotlin suite now runs under 2.0.21 too (ONBOARDING row) |
| **G1 React Native runtime** (ADR-038): `@undra/react-native` TurboModule over the C ABI under the TS mirror, `undra build --platform rn`, the playground RN app, a `react-native` CI job + `rn-devices.yml` (simulator/emulator on PRs touching RN, weekly), `scripts/rn-device-checks.sh` | `6fe1643` | opus review `.10x/reviews/2026-10-01-react-native-review.md`: ownership trace holds, 2 Medium fixed (a failed second start froze the running core; a stopped runtime kept calling the new core), 11 Low fixed; 10/10 on-device checks on the iPhone 17 Pro simulator and the `undra-rn` emulator; 20 Android reloads with flat heap; limits documented in `docs/REACT_NATIVE.md` (RN 0.87 New Architecture, one instance per process until ADR-044, ~10k patches/s on Hermes — E4, app supplies adapters — G1b open) |

Matrix at checkpoint 7: Rust 2,351 · TS 1,049 · Kotlin 585 · Swift 467 · RN 41 + 14/14 C++ · contracts 54/54.

### Checkpoint 8 (2026-10-01, evening) — Android adapters landed

| Piece | Merge | Verdict |
|---|---|---|
| **Android platform adapters**: `AndroidPlatformDefaults.install(core, context)` registers Kv, SecureStore (Keystore-sealed), Fs (root-guarded), Http, Connectivity and Lifecycle; the playground and the `undra init` template use it instead of the fakes; JVM `FsAdapter.delete` recursive, `FileKv.list` header-only; typed port errors stay typed (`HttpError.Network` offline), untyped adapter failures reach `onError` as `Malformed`; SPEC §8 Fs semantics; `docs/ERRORS.md` rows | `429fb9f` | opus review `.10x/reviews/2026-10-01-android-adapters-review.md` + cross-merge record in `.10x/decisions/sde/android-adapters.md`; Kotlin 588 (brew 2.4.20 and CI's 2.0.21), adapters 130/131 JVM, **112/113 instrumented on the `undra` AVD**, `smoke.sh` passed (offline queue, replay with the idempotency key), remote mode against `undra dev` reinstalls the adapters after a session loss; contracts 54/54; Rust 2,351. Open: M1 Maven publishing; remote cores drop Connectivity/Lifecycle reports made while the connection is down (dev only); F3–F6 |

Matrix at checkpoint 8: Rust 2,351 · TS 1,049 · Kotlin 588 · Swift 467 · RN 41 · contracts 54/54.

### Checkpoint 9 (2026-10-01, night) — Track A landed: the runtime lifecycle

| Piece | Merge | Verdict |
|---|---|---|
| **Track A** — ADR-034 `WeakCtx`/`Gone`/`Ctx::closed()`, a runtime ends when its owner lets go (Kotlin `close()` ends an in-process core; `runtime_threads` in stats); ADR-035 off-core writes refused in every build (E0065, `try_set` → `WriteError::OffCore`, change-sets routed to the owning runtime); ADR-019 amendment: a panicking computed poisons only itself; ADR-036 typed stream errors (flag 2 carries `E`, flag 3 `StreamFailure{status,message,detail}`, the text-guessing stop-gap removed on Kotlin/TS, status 5 → `Refused` on all three); macros accept `Stream<Item = Result<T,E>>`; S07.6/S07.7/S17.7; ADR-034 Amendment A (what a call pins) | `2186bad` | opus review `.10x/reviews/2026-10-01-runtime-lifecycle-review.md`: merge after fixes; no High; M1 fixed (S17.7 could not fail — `runtime_threads == 0` asserted after every close, proven with a mutant), M2 documented as the amendment; JNI shutdown raced under Miri; "answered once" raced in release; the write checker compares runtime ids, 8.5 ns vs 65 ns; wire version stays 1 per ADR-036/Amendment C. Rust 2,400 · TS 1,102 · Kotlin 612 (2.4.20 and 2.0.21) · Swift 480 · RN 45 · wasm 19+24 · contracts 54/54; playground hash `0xddcdea47fa95a8d4`. Open Lows: L2–L5, L8, TS unknown-flag path |

Matrix at checkpoint 9: Rust 2,400 · TS 1,102 · Kotlin 612 · Swift 480 · RN 45 · contracts 54/54.
In flight: `wasm-size` (E5, review), `tooling` (D2–D5, review), `dev-reload` (B3, ADR-053 accepted), `docs-reference` (H3), `rn-adapters` (G1b).
Unblocked now that Track A is in: `abi-table` (ADR-044), `persistence-v2` (ADR-037/049), `derived-lists` (ADR-039), E4, `ts-runtime-size`, `testkit`, the Rust 1.99 bump.

### Checkpoint 10 (2026-10-01, night) — the API reference

| Piece | Merge | Verdict |
|---|---|---|
| H3 API reference: rustdoc for `undra` + the six re-exported crates built in the site workflow with `-D warnings` and published at `/reference/rust/` (site palette and Geist laid over rustdoc's theme); `/reference/{swift,kotlin,typescript}.html` generated from the committed playground bindings by `site/scripts/build-reference.mjs` (declarations only, parser `decls.mjs` with tests, collapsed per file), covered by the "generated files up to date" check | `a22b8ed` | fable review (screenshots, desktop + phone, dark + light); open: a custom port in the playground so the Ports section has an example; `undra bindgen --declarations` would replace the parser; rustdoc has no link back to the site |

### Checkpoint 11 (2026-10-02) — tooling landed

| Piece | Merge | Verdict |
|---|---|---|
| **D2–D5 tooling**: `undra doctor` with 34 checks (`--fix` prints, `--json`), Gradle `undraBuild` before `preBuild`, an Xcode Run Script phase with per-file inputs and a per-configuration stamp, `@undra/runtime/vite` (zero deps; skips vitest), `undra init` emits `.github/workflows/undra.yml`, `undra upgrade` moves every pin in lockstep with migration notes (atomic, refuses newer projects with C0014); release builds remap the builder's home path | `aa66ce9` | opus review `.10x/reviews/2026-10-01-tooling-review.md`: merge after fixes; 6 Medium fixed (a Swift URL match that caught `undra-charts`; non-atomic upgrade; the shim's stale `Cargo.lock`; vitest building the core; doctor fixes that did not run in the printed shell; false migration notes), 11 Low; debug→release→debug proven on Gradle and Xcode; Rust 2,530 · TS 1,128 · Kotlin 612 · Swift 480 · contracts 54/54. Open: first post-merge CI run (the android job's Gradle step can skip silently); version catalogs; the `0.1.0` migration key at 1.0.0; no RN scope in doctor |

Matrix at checkpoint 11: Rust 2,530 · TS 1,128 · Kotlin 612 · Swift 480 · RN 45 · contracts 54/54.

### Checkpoint 12 (2026-10-02) — the web bundle under budget

| Piece | Merge | Verdict |
|---|---|---|
| **E5 wasm-size** (ADR-052 Accepted): the hello-world web core goes from 136.2 KB to **102.7 KB gzipped** (budget 120 KB) by one stable sort routine for the schema code (16 monomorphised copies gone) and `undra-query` linked only into cores that declare a query or mutation; `scripts/wasm-size.sh` records to `bench/results/web-size.jsonl`, `[size]` tables in `budgets.toml` gate the wasm and the JS runtime (24.8 KB against a restated 26 KB; `ts-runtime-size` targets 16 KB), a `size` job in `bench.yml`, and `build-numbers.mjs` fills README/site/posts from the record; release builds remap the builder's home path | `2b7534d` | opus review `.10x/reviews/2026-10-01-wasm-size-review.md`: merge after fixes; M1 the query bench had broken (now run in CI), M2 hand-written queries were never hydrated, M3 home paths in every release binary, M4 the JS gate; the sort costs ≈2 µs at cold start, paid for by lever B; Rust 2,556. Open: binaryen tarball sha256 not pinned |

Matrix at checkpoint 12: Rust 2,556 · TS 1,128 · Kotlin 612 · Swift 480 · RN 45 · contracts 54/54.

### Checkpoint 13 (2026-10-02) — state-preserving reload

| Piece | Merge | Verdict |
|---|---|---|
| **B3 dev-reload** (ADR-053 Accepted): on a successful rebuild `undra dev` starts the new core in standby, suspends the old server (2 s settle), snapshots it (memory only, 16 MiB cap), restores before the new core listens, hands the ADR-051 session over; the dev bar says "Reloaded, state kept" or "state reset: <reason>"; `onDevNotice` last in every runtime signature, dev-only; `--no-keep-state`; a Kotlin `RemoteTransport` `Hello` race fixed | `e119d4c` | opus review `.10x/reviews/2026-10-02-dev-reload-review.md`: merge after fixes; M1 a call made during the swap vanished silently (now counted and shown: "N calls lost in the reload"), M2 a core could speak as the dev server (target filtered at the bridge); runner stdout bounded; the generation floor asserted across processes; proven on the `undra` AVD, iOS simulator and web; snapshot 204 KiB / restore 1.6 ms for the playground; Rust 2,603 · TS 1,132 · Kotlin 616 · Swift 482 · contracts 54/54. Open: query handles do not survive a reload (needs its own ADR); L3/I1/I4 |

Matrix at checkpoint 13: Rust 2,603 · TS 1,132 · Kotlin 616 · Swift 482 · RN 59 · contracts 54/54.

### Checkpoint 14 (2026-10-02) — React Native adapters, the Swift Fs fix

| Piece | Merge | Verdict |
|---|---|---|
| **G1b rn-adapters** (ADR-038 Amendment B): all ten standard ports by default in `@undra/react-native` — Kv/Fs in one portable C++ store on per-port workers, Keychain/Keystore SecureStore, native connectivity monitors, Http over `fetch`, `AppState` lifecycle; a `platform` module in the playground core; RN11–RN20 device checks | `15afd10` | opus review `.10x/reviews/2026-10-02-rn-adapters-review.md`: merge; M1 SPEC rows, L1 a Keychain key with U+0000 listed cut short, L2 a second JS connectivity source beside the native one, six Lows (OOM `terminate`, pending JNI exception, thread names, leaks); a stopped core's late reply never reaches the next core (proven); 19/19 iOS, 20/20 Android; RN 60 |
| **swift-fs**: `FsAdapter` walks from the root descriptor with `openat(O_NOFOLLOW)` (symlinks out of the root were followed), atomic writes, `KvAdapter.list` skips temp and damaged files | `262f38a` | small fix piece; 22 + 7 tests, mutants fail; Swift 511; open: a damaged entry is "no value" in Swift and "unavailable" in the C++ store — unify under ADR-049's `StorageError` |

Matrix at checkpoint 14: Rust 2,603 · TS 1,132 · Kotlin 616 · Swift 511 · RN 60 · contracts 54/54.
In flight: `derived-lists` (opus review), `abi-table`, `persistence-v2` (finishing), `ports-v2` (ADR-047/048 → implementation), `devtools` (ADR-054), `testkit` (ADR-055).

### Checkpoint 15 (2026-10-02) — derived keyed lists

| Piece | Merge | Verdict |
|---|---|---|
| **E2 derived-lists** (ADR-039 Accepted): `DerivedList<T>` / `derive()` with filter/map/sort (+ parameters) and `count()`, two arena order-statistic trees, source taps (4,096 cap), one patch per transaction, the 256-op parameter re-walk; the playground's `Todos` on recorded ops with derived `visible`/`remaining`; S19 replays a 60,000-op seeded recording on all columns; nine budget rows, two ratios, a stress scenario; a landing card "Filtered view of a 10,000-row list, one row changed" (392 ns, 158 B — was 176 µs, 353 KB) | `76f9364` + `(wording)` | opus review `.10x/reviews/2026-10-02-derived-lists-review.md`: sound, no High/Medium; a second model (four views + a computed, tap overflow inside one transaction) 3 × 3,000 cases; the hello-world wasm unchanged (a core without derived lists does not link the index); L1 S19 on the RN column, L2 patch merging pinned on Kotlin/Swift, L4 read-your-writes documented; Rust 2,700 · Swift 512 · Kotlin 617 · TS 1,132 · RN 60 · contracts 57/57 (19 × 3); playground hash `0xc5f05c376fde398c` |

Matrix at checkpoint 15: Rust 2,700 · TS 1,132 · Kotlin 617 · Swift 512 · RN 60 · contracts 57/57.

### Checkpoint 16 (2026-10-02) — the ABI function table

| Piece | Merge | Verdict |
|---|---|---|
| **abi-table** (ADR-044 Accepted): the C ABI is a v2 function table (`UndraApi`: version, size, hash, namespace, 17 entry points) exported once per core as `<ns>_undra_api` by `export_core!`, `JNI_OnLoad` registers natives on the generated class, no named exports; `[core] namespace` (lowercase ≤ 32, reserved names refused), iOS prelinked `lib<ns>.a` with one global symbol, `lib<ns>.so`, `<ns>.wasm`; bindgen `Undra<Ns>` entries in all three languages; runtimes claim cores per namespace; the RN host finds cores by table; `examples/two-cores` + `two-cores.yml` + S26; `boundary/call_sync/add` +0.6 ns median; no schema hash change; Amendment A: default storage directories per namespace (follow-up `ns-storage`) | `2078268` | opus review `.10x/reviews/2026-10-02-abi-table-review.md`: sound; M1 reserved entry names, M2 a sibling checkout was refused; two cores under ASan (A shut down while B streams; A's late reply ignored); `nm` shows exactly one global per core; Rust 2,718 · Swift 527 · Kotlin 630 · TS 1,133 · RN 65 · contracts 60/60 (S01–S19, S26 × 3); hello-world wasm +0.2% |

Matrix at checkpoint 16: Rust 2,718 · TS 1,133 · Kotlin 630 · Swift 527 · RN 65 · contracts 60/60.

### Checkpoint 17 (2026-10-02) — devtools and persistence

| Piece | Merge | Verdict |
|---|---|---|
| **B4 devtools** (ADR-054): a page served by `undra dev` at `/devtools` behind a 128-bit per-run token (every refusal the same 404), server-side observe-all through a hub with the app's observed set restored after, a time-travel ring (200 steps / 32 MiB / 4 MiB) restoring through §5.9 with the app converging, port and query logs, counters; `Runtime::register_inspector` is the one runtime seam; assets 17 KB gzipped, rebuilt-equal in CI | `0303369` | sonnet review `.10x/reviews/2026-10-02-devtools-review.md`: sound with fixes; M1 the WebSocket refusal differed from the 404, M2 a panicking inspector was silent and re-called, M3 the query cache sampled under its lock; Rust 2,789 · contracts 60/60 |
| **persistence** (ADR-037 + ADR-049 Accepted): snapshot layout 2 with type fingerprints, the migrating restore (`restore_with_report`, code 7 `INCOMPATIBLE`), `#[undra::migrate]` and `#[undra(default)]` with E0066, undra-query format 2 with dead letters; `Kv`/`SecureStore` return `Result<_, StorageError>` on every platform incl. RN, `FsError::{Full, Unavailable}`, the queue never overwritten while unreadable; TS worker protocol 3 with `worker.ports`; `crashRecovery()` restarts a trapped web core from its last snapshot; S20–S22; the stdlib dispatchers linked by use (hello wasm 116.8 KB); dev-reload now carries state across a migratable schema change | `bd1987b` | opus review `.10x/reviews/2026-10-02-persistence-review.md`: sound after fixes; H1 a wrong-typed migration hook spliced bytes into a signal, H2 React Native storage was not on ADR-049, H3 a trap during a restart left a dead core; M1–M4; a damaged-snapshot fuzz at every byte, a differential test of the hand-written JSON; Rust 2,891 · Swift 553 · Kotlin 652 · TS 1,264 · RN 65 · contracts 65/65; hash `0xfa536b9ac6f06149`. Open: JS runtime at 25,984/26,000; RN SecureStore failures all `Io`; crafted snapshots reach O(n²) sorts |

Matrix at checkpoint 17: Rust 2,891 · TS 1,264 · Kotlin 652 · Swift 553 · RN 65 · contracts 65/65 (S01–S22 + S26).
In flight: `testkit` (complete; sonnet review + cross), `ports` (complete; opus review + cross).

### Checkpoint 18 (2026-10-02) — the testing kit

| Piece | Merge | Verdict |
|---|---|---|
| **F1+F2 testkit** (ADR-055): `PreviewCore` (the real core in-process with platform-native fakes and a manual clock) and `RecordedCore` on Swift (`UndraTestKit`), Kotlin (`dev.undra:testkit`), TS (`@undra/testkit`, zero deps) and Rust (`undra::testing`); `undra dev --record` via a transport `FrameTap` (SecureStore redacted by default); one `undra.recording` v1 format written byte-identically by all four; a fakes conformance file generated by the Rust fakes and replayed on every kit; playground previews on iOS/Android/web built in CI; `docs/TESTING.md` + a site page | `4d222e4` | sonnet review `.10x/reviews/2026-10-02-testkit-review.md`: sound with fixes; M1 `advance` could loop forever on a self-re-arming timer (typed `TimerStorm` cap), M2 secrets in recordings, M3 duplicate call ids from two recorders, M4 an unwritable recording grew silently; conformance mutations fail on all four kits; Rust 2,940 · Swift 581 · Kotlin 682 · TS 1,264 + 32 · contracts 65/65 |

Matrix at checkpoint 18: Rust 2,940 · TS 1,264 · Kotlin 682 · Swift 581 · RN 65 · contracts 65/65.
In flight: `ports` (review resuming: B1 size fix, sub-reviews, matrix), `docs-v1x` (cookbook + sample).

### Checkpoint 19 (2026-10-02) — the cookbook and the sample

| Piece | Merge | Verdict |
|---|---|---|
| **H1+H2 docs-v1x**: `site/docs/cookbook/` (auth, pagination, forms, upload, real-time, offline-first, from KMP, from UniFFI; code filled from the crate by `build-cookbook.mjs`), `examples/cookbook` (seven recipe modules, 35 tests + 9 behind `realtime`; 15 platform snippets compiled with `swift build`/`kotlinc -Werror`/`tsc --strict`), `examples/fieldbook` (field notes with photos: sign-in with refresh, local-first notes, a derived view, Fs photos, every change a queued idempotent mutation, a build 1→2 migration, state kept across dev-reload, presence behind `presence`; web + iOS + Android shells, previews on the testing kit; 18 core tests, 13 web) | `61f9da0` | fable check (structure matches the docs pages; 375 px clean; 37 pages link-clean; 342/350 words). Open: flip `realtime`/`presence` features and remove `site/data/pending.json`'s entry when ports lands; Fieldbook on `Kv` until the Db port lands; the two migration pages need the H4 fact-check |

Matrix at checkpoint 19: Rust 2,993 · TS 1,264 + 32 · Kotlin 682 · Swift 581 · RN 65 · contracts 65/65.
In flight: `ports` (review resuming).

### Checkpoint 20 (2026-10-02) — ports: WebSocket, SSE and Db

| Piece | Merge | Verdict |
|---|---|---|
| **ports** (G2+G3, ADR-047/048 Accepted): `WebSocket`, `Sse` and `Db` standard ports behind cargo features (standard hash unchanged `0xbbf6f70d0c567f47`), inbound as a batched pull whose `max` is the credit, typed ends, interactive transactions with a 5 s `Busy` deadline, SQL as `&'static str`; adapters on Swift (`URLSessionWebSocketTask`, SQLite3), Kotlin JVM (own RFC 6455 client, `java.net.http`, JDBC), Android (`android.database.sqlite`), TS (`@undra/runtime/realtime`, `/db` with wa-sqlite in a worker on OPFS), RN (native C++ binding); the playground's Notes and Live tabs; S23–S25; bench rows under budget; the cookbook's `realtime` and Fieldbook's `presence` features on | `50fd54d` | opus review `.10x/reviews/2026-10-02-ports-review.md`: sound after fixes; B1 the hello-world JS 132 B over its gate (`OptInPortIds` moved to the subpaths, dispose on close), H1 a cancelled `connect`/`open`/`begin` orphaned what the platform opened (`owned.rs`), H2 a crash restart kept the trapped instance's connections, H3 two opens ran migrations twice on every binding, H4 Android stuck after a refused `COMMIT`; Rust 3,052 · Swift 668 · Kotlin 753 · TS 1,432 · RN 87 · contracts **74/74**; hello wasm 116.6 KB, JS 25,996/26,000; playground hash `0xb5b7b1dc29182a9d`. Open: a migration containing its own `COMMIT`; the web Db serves one tab per origin |

Matrix at checkpoint 20: Rust 3,052 · TS 1,432 + 32 · Kotlin 753 + 30 · Swift 668 · RN 87 · contracts 74/74 (S01–S26 incl. S23–S25).
In flight: `objects-callbacks` (ADR-040/041), `ios-floor` (ADR-045), `prod-ops` (ADR-046).

### Checkpoint 21 (2026-10-02) — the dev-reload flake, iOS 15/16

| Piece | Merge | Verdict |
|---|---|---|
| **dev-reload-flake**: the integration tests' raw client never answered the server's pings during a long rebuild, so the server dropped it as dead at 15 s under load; the client now has a reader thread like a runtime's, plus a transport test that a late reader still finds the Close frame first | `a3c4b93` | 90/90 loaded runs (load 22–83); contracts 74/74; the server still logs no reason when it drops a dead peer (open) |
| **ios-floor** (ADR-045 Accepted): `[ios] deployment_target` below 17 generates `ObservableObject` + `@Published` stores (default output byte-identical), the runtime's floor is iOS 15 / macOS 12 with `UndraConnectionObject` as the pre-17 twin and `UndraDuration` in nanoseconds, `undra init --ios-deployment-target 15.0`, `examples/ios15-sample`, the Swift contract grid in floor mode, an `ios-floor` CI job with a runtime probe | `e69ff1c` | sonnet review `.10x/reviews/2026-10-02-ios-floor-review.md`: sound with fixes; M1 a weak capture left the iOS 17 connection object stale, M2 the config diagnostics lost their why; `@Published` copies the array once per patch (48 µs at 10k rows, documented); 24/24 at the floor and the default; Swift 674; Rust 3,078; proven by compilation and a 26.5 runtime probe, no 15/16 runtime installed |

Matrix at checkpoint 21: Rust 3,078 · TS 1,432 + 32 · Kotlin 753 + 30 · Swift 674 · RN 87 · contracts 74/74.
In flight: `ns-storage` (review), `objects-callbacks`, `prod-ops`, `ts-size-e4`.

### Checkpoint 22 (2026-10-02) — the web call path, per-namespace storage

| Piece | Merge | Verdict |
|---|---|---|
| **ts-size-e4** (ADR-056 + an ADR-052 amendment): the JS gate measures the chunk loaded up front (lazy chunks reported); the remote transport and the four default ports load on first use; the call path allocates nothing on the hot path (byte-level writer/reader, one-allocation call payload, a direct call that returns a settled promise, a shared scratch `DataView`, optional `Transport.sendCall`/`callSyncParts`), the Vite plugin defaults `build.target` to es2022; web budgets as tests (`[web."id"]` rows read by the device bench and a runtime test) | `ab7c6b3` | sonnet review `.10x/reviews/2026-10-02-ts-size-e4-review.md`: sound with fixes (M1 a throw after a sync answer rejected the call, M2 an explicit Timer adapter on a remote core registered too late, M3 flaky RN waits); ordering under 46 cases + a mutant; a 12,000-payload differential wire fuzz against main; Chromium: handle call 3.2 µs → 0.47 µs, callSync 3.6 → 0.3 µs, 100-signal change-set 88 → 17 µs, frame 4.0 → 1.1 ms; Hermes mirror work per frame at 100k patches/s 22 → 5 ms; hello JS 25,996 → 21,173 (gate 21,500; a hello app that calls Kv loads 25,318 over its life); 16 KB not reachable without removing required behaviour (recorded) |
| **ns-storage** (ADR-044 Amendment A as built): every default store under `…/undra/<ns>/…` (Apple `Undra/<ns>/`), Keychain/Keystore items prefixed, browser names `undra.<ns>.*`, the namespace from the generated entry at load and validated on every host before it becomes a path; two cores write to distinct places on iOS, Android, JVM and Node | `1625dfe` | sonnet review `.10x/reviews/2026-10-02-ns-storage-review.md`: F1 an unvalidated namespace path component on every host (fixed), F2 the bindgen TS fixtures (fixed); JS 21,336 of 21,500 after crossing ts-size-e4; TS 1,580 · Swift 686 · Kotlin 756 · adapters 147 instrumented · RN 88 · contracts 74/74 |

Matrix at checkpoint 22: Rust 3,079 · TS 1,580 + 32 · Kotlin 756 + 30 · Swift 686 · RN 88 · contracts 74/74.
In flight: `objects-callbacks` (ADR-040/041, final matrix), `prod-ops` (ADR-046, final matrix); drafted: `default-choice-post` (fact-check pending).

### Checkpoint 23 (2026-10-02) — objects as parameters, host callbacks

| Piece | Merge | Verdict |
|---|---|---|
| **objects-callbacks** (ADR-040 + ADR-041 Accepted): handles are 24-bit slot + 40-bit generation with a `u64` floor; `TypeRef::Object`/`Callback`; `Arc<T>`/`&T`/`Option`/`Vec<Arc<T>>` parameters and returns, `Arc<Self>` constructors; the object table counts host references, interns by address, an RAII `IssueScope` gives back what a reply did not carry; one wrapper per handle on every platform; a foreign core's handle refused host-side; `#[undra::callback]` traits as ports with instances (`main` delivery through the mirror drain, `background` on a serial executor), typed errors status 1, anything else `onError` + status 2, cancellation to `Task`/`Job`/`AbortSignal`; the playground's Workshop tab; S27/S28 on all columns; bench rows (object param 48.7 ns ≈ `add`; return 84 ns; callback round trip 183 ns) | `848d3a7` | opus review `.10x/reviews/2026-10-02-objects-callbacks-review.md`: merge; F1 (High) a collected wrapper's handle returned later threw in TS, F2 restore reset host refs, F3 a return racing shutdown answered status 0, F4 the callback registry survived a crash restart, F5 Swift ran `onCancel` under the core lock; the JS gate restated at 21,800 (record 21,672; lazy-loading the identity map would cost 2.6–3.8 ms on the first `create()`); the stress-screen smoke failure was a stale build; O1–O10 to `objects-followups`; Rust 3,133 · Swift 705 · Kotlin 782 · TS 1,613 · RN 92 · contracts 80/80 (S01–S28) |

Matrix at checkpoint 23: Rust 3,133 · TS 1,613 + 32 · Kotlin 782 + 30 · Swift 705 · RN 92 · contracts 80/80.
In flight: `prod-ops` (ADR-046, opus review), `types-paging` (ADR-042/043), `objects-followups` (O1–O8), `default-choice-post` (drafted).

### Checkpoint 24 (2026-10-02) — production operations

| Piece | Merge | Verdict |
|---|---|---|
| **prod-ops** (ADR-046 Accepted, 19-point amendment): every contained panic is a FATAL `undra::panic` record plus one `PanicReport` (message, location, operation, thread, frames with image-relative addresses, namespace, core version, schema hash, image id) to the standard `Diagnostics.panicked` port, delivered to `onPanic` in order on every platform (wasm builds it host-side from the trap); `run_background(deadline)` runs per-runtime tasks (the query client's replay/refetch/flush) with iOS `UndraBackground`, Android `android-work`, the web page window; the CLI keeps line-table symbols, writes `build/symbols/manifest.json`, `undra symbolicate`, `.lldbinit`; S29/S30 | `4d6effd` | opus review `.10x/reviews/2026-10-02-prod-ops-review.md`: sound with fixes; H1 the hello wasm went over its gate after the cross (the panic path is linked by use on wasm — `guarded()` is `Ok(f())` under `panic=abort`; 119,654 of 120,000), M1 two `SAFETY` claims held only for images `undra build` makes (bounded ELF/Mach-O reads with the real load bias, Miri-checked), M2 two containment sites reported nothing, M4 an empty image id right after load; the JS gate reconciled at 22,100 (main 21,336 + objects 336 + prod-ops 333, `runInBackground` and the Diagnostics registration lazy); symbolication proven on iOS Release, Android, host and web (`lab.rs:222/221/236`); Rust 3,229 · Swift 772 · Kotlin 810 · TS 1,681 · RN 104 · contracts 86/86; hash `0xcc36d9fa84455aef` |

Matrix at checkpoint 24: Rust 3,229 · TS 1,681 + 37 · Kotlin 810 + 32 · Swift 772 · RN 104 · contracts 86/86 (S01–S30).
In flight: `types-paging` (ADR-042/043), `objects-followups` (O1–O8); drafted: `default-choice-post`.

### Checkpoint 25 (2026-10-02) — newtypes, generics, Decimal; paged and lazy lists; polling

| Piece | Merge | Verdict |
|---|---|---|
| **types-paging** (ADR-042 + ADR-043 Accepted): newtypes cross transparently with native wrappers (Swift struct, Kotlin value class, TS branded type), `#[undra::api(generic)]` instantiations (E0070), `Decimal` as a 16-byte i128 mantissa + scale ≤ 38, leaf features (`uuid`, `chrono`, `time`, `rust_decimal`, `bytes`) with a CI job; infinite queries (`fetch_next_page`, `has_next_page`), `Lazy<T>` lists with a transient page server and `Lazy::over(&derived)` (Swift `UndraLazyList` + pre-17 twin, Kotlin `UndraLazyList` + `undra-compose`, TS `useLazyList`), polling with pause on Background/offline; the playground's Ledger, Library (10,000 lazy rows), Feed and Ticker screens; S31–S33; ADR-031 amended (a lazy invalidation supersedes only earlier invalidations of its signal) with model tests in all three mirrors | `686a983` | opus review `.10x/reviews/2026-10-02-types-paging-review.md`: sound with fixes; H1 Kotlin/Swift saturated any mantissa over 127 bits to i128::MAX (off by 38 orders), M1 page sizes over 4,096 broke the list on every platform, M2 the TS mirror applied an invalidation while waiting for a full value; mutants of the amendment fail in each mirror; Rust 3,518 · Swift 862 · Kotlin 880 · TS 1,847 · RN 109 · contracts **95/95** (S01–S33); hello wasm 116,480, JS 22,068 of 22,100; hash `0xaa836fffb918e598`. Open: L2–L6 (refused page reporting differs, retry limits differ, poll resume timing, `undra-compose` tests not in CI, `rust_decimal` scale 28) |

Matrix at checkpoint 25: Rust 3,518 · TS 1,847 + 37 · Kotlin 880 + 32 · Swift 862 · RN 109 · contracts 95/95.
Every ADR from 029 to 056 is implemented and merged. In flight: `objects-followups` (review + the path-independent size gate); drafted: `default-choice-post`.

### Checkpoint 26 (2026-10-02) — the objects follow-ups; every code piece of the v1.x program is merged

| Piece | Merge | Verdict |
|---|---|---|
| **objects-followups** (O1–O8 of the objects review): streams that take objects or callbacks lend and give back on every platform, one Swift wrapper per handle with cleanup under the identity map's lock, restore cancels calls holding a replaced store as a parameter, TS crash-restart replay distinguishes give-backs from releases, per-origin dev-session accounting, a constructor failing after taking callbacks answers status 2 without a double release, an abort racing a success reply reclaims the reference on every transport, Kotlin `invoke` for every `new`, the Swift weak wrapper fails rather than hangs; release builds remap `~`, `CARGO_HOME`, the registry, the checkout and the project so the module does not depend on where it was built | `14aae90` | sonnet review `.10x/reviews/2026-10-02-objects-followups-review.md`: merge; F1 (High) a pre-existing deadlock in Swift's identity map (a weak load under the lock deallocating a wrapper whose `deinit` takes the lock — found by an 8-thread stress), F3 StrictMode double effects closed a live object, F4/F5 parameter-slot gaps; Rust 3,536 · Swift 870 · Kotlin 881 · TS 1,856 · RN 110 · contracts 95/95; hash `0xcaec1b9d8ea1f199`; wasm 116,690 of 120,000, JS 22,100 of 22,100 (zero headroom: the next JS change makes room or restates ADR-052) |

Matrix at checkpoint 26: Rust 3,536 · TS 1,856 + 37 · Kotlin 881 + 32 · Swift 870 · RN 110 · contracts 95/95 (S01–S33).
**Every code piece of the v1.1/v1.2 program is merged.** Left: the `default-choice-post` fact-check and publication, the Rust 1.99 bump (needs `rustup update stable`), the final state pass.

### Checkpoint 27 (2026-10-02) — the post is published

| Piece | Merge | Verdict |
|---|---|---|
| **default-choice-post** (H4): "Why Undra is the default choice" — a 36-row limitations matrix against KMP, UniFFI and Flutter/React Native-as-logic (Undra: 27 solved, 6 partial, 2 open, 1 by decision), five measured sections, the adoption cost, what is open; `claims.md` public beside it | `0654810` | opus fact-check `.10x/reviews/2026-10-02-default-choice-post-fact-check.md`: publish; 221 draft claims → 182 verified, 29 corrected (8 about other tools), 10 removed, 11 added; all 45 linked competitor pages re-read from source (plus the 68 pages of UniFFI's manual for the "describes no …" claims); no speed comparison with another tool; the stale statements in the reads post, the KMP post, both migration guides, `roadmap.json` and `pending.json` fixed in the same pass |

In flight: `ci-green` (every red CI job at its root cause + the Rust 1.99.0 bump; the local toolchain is 1.99.0 since `rustup update stable`), `landing-refresh` (landing page, roadmap page, docs nav, README to what shipped).

### Checkpoint 28 (2026-10-02) — the landing page, roadmap, docs nav and README match what shipped

| Piece | Merge | Verdict |
|---|---|---|
| **landing-refresh**: the hero names React Native; one "new" row under the hero (React Native · Devtools with time travel · Derived lists · WebSocket, SSE and Db ports · Migrations · iOS 15/16 · the post · the roadmap); the badge "v1 shipped · v1.x on main" (no release claimed; brew/npm/curl still wait for the tag); the measured note says simulator, emulator and Chromium rows exist and real-phone rows do not; the tests card and the contract grid generated by `site/scripts/build-trust.mjs` (7,253 tests; 33 scenarios, 95/95 cells); the harsh-conditions cards read `bench/results/*.json`; the Android core card re-measured (978.6 KB arm64-v8a, recorded in `bench/results/android-size.jsonl`); cold start 85 µs; the collapsed samples match main's API; the roadmap re-verified item by item (Now = open items, Next = release and reach); docs nav regrouped with no orphans; README features, numbers from records, an honest "not done" | `1fb8db8` | fable check (screenshots dark/light at 1280 and 375: the v1 look intact); landing prose 347/350; 47 pages link-clean; the live demo and "Push it" verified. No web call-path card by decision (the only budget is the CI gate; against the design target it is ~6× over, which the post states) |

In flight: `ci-green` (every red CI job at its root cause, the Rust 1.99.0 bump, the Site workflow's install step, re-recorded sizes).
