# Keel v1 — status

Updated: 2026-09-30 (integrator takeover, branch `claude/keel-framework-takeover-66c4ea`)

## Phase

Implementation (Phase 4 of the 10x flow). Strategy/design live in `docs/blueprint.html`
and `docs/SPEC.md`; the plan of record is `docs/HANDOFF.md`.

## Test truth (all green as of the update above)

| Suite | Result |
|---|---|
| Rust `cargo test --workspace` | 1,763 passed / 0 failed |
| TS `npm test` (runtimes/ts/@keel/runtime) | 831 passed (21 files) |
| Kotlin `scripts/test-local.sh` | 454 cases, 0 failed (JNI smoke passes against libkeel_ffi) |
| Swift `swift test` (needs full Xcode; env.sh sets DEVELOPER_DIR) | 313 passed / 0 failed |

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

- keel-runtime review fixes (H1 restore generations, M1-M3, lows; ADR-022) — `wt/runtime-fixes`.
- keel-macros review fixes (H1 schema-identity checks, H2 typed port errors; ADR-023) — `wt/macros-fixes`.
- bindgen standard-library filter (ADR-024) — `wt/stdlib-filter`.

## Landed since takeover

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
