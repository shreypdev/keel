# Undra v1 — integrator's handoff

Reconstructed 2026-09-30 on the takeover branch after merging the four outstanding
branches (`wt/integrate`, `wt/ts-core`, `wt/kotlin-core`, `wt/swift-core`). The original
HANDOFF was never committed; this file is now the living plan of record. Keep it current.

## 1. Where the work stands

Landed and green on this branch:

| Piece | State | Proof |
|---|---|---|
| `undra-meta` | done | schema model, canonical JSON + hash, FNV, inventory registration |
| `undra-wire` | done | full §3 codec, envelope, proptest + byte-fuzz |
| `undra-macros` | done | api/error/store/port/query attrs, dispatch, diagnostics §12, UI tests |
| `undra-signals` | done | Signal/Computed/Effect, StoreCell + CellSlot, txn commit, keyed diff |
| `undra-runtime` | done | executor, core lock, object table, change-sets, ports, timers, snapshot |
| `undra-bindgen` | done | Swift/Kotlin/TS generators, golden files, TS typecheck+run, Kotlin compile+run |
| `undra` facade | done | prelude, re-exports, `query.rs` contract traits, e2e todo test |
| TS runtime | done | 830 vitest cases: core, mirror, signals, stores, ports, transports, adapters, wasm stub |
| Kotlin runtime | done | 454 local-runner cases; JNI smoke suite waits on `undra-ffi` (2 skips) |
| Swift runtime | done (code + tests) | compiles under Swift 6.3; XCTest suite runs with full Xcode |
| Rust suite | 1,079 passing | `cargo test --workspace` |

Not yet built (stubs): `undra-ports`, `undra-query`, `undra-ffi`, `undra-transport`, `undra-cli`,
`examples/playground` (core is a stub, no apps), `bench` (empty harness), contract-tests
runners (only `wire-vectors.json` exists), CI workflows.

## 2. Ordered remaining work

Each piece in its own worktree (`scripts/wt.sh new <slug>` → `.work/<slug>`, branch
`wt/<slug>`), implemented from a precise brief, adversarially reviewed before merge.

1. **undra-ports** (SPEC §8) — the ten standard ports with `#[undra::port]`, the §8 records,
   and all fakes + `TestRuntime` glue. The record layouts and ids are *already implemented*
   by the three platform runtimes' adapters; the Rust side must match them, not the other
   way round. Cross-check against `runtimes/*/…/adapters` and the wire vectors.
2. **undra-query** (SPEC §9) — QueryClient, QueryHandle store object (signals data/status/
   error/fetching/updated_at), refetch triggers, retry w/ jitter, dedup, mutations with
   optimistic patches + rollback, offline queue, persistence. Implements the traits in
   `crates/undra/src/query.rs`. Handle method ids `0x21d1b9e2` refetch / `0x44cec2fa`
   invalidate (already baked into the runtimes).
3. **undra-ffi** (SPEC §6, §6.1, §7) — C ABI, JNI shim (`dev.undra.runtime.UndraNative`,
   RegisterNatives via JNI_OnLoad), wasm exports/imports exactly as
   `runtimes/ts/@undra/runtime/src/transport/wasm-main.ts` consumes them. Unsafe only here,
   every block `// SAFETY:`. The Kotlin NativeSmokeTests and the TS wasm tests are the
   acceptance tests.
4. **undra-transport** (SPEC §3.2, §5.10) — WebSocket server for `undra dev` (tungstenite),
   envelope framing, Hello/schema check, remote transport contract the three runtimes
   already test against fake servers.
5. **undra-cli** — init/build/dev/adopt/doctor/bindgen; XCFramework/AAR/npm packaging;
   schema extraction by dlopen (§13).
6. **examples/playground** — one Rust core (todos, counter, 10k keyed list, a query,
   benchmark methods); React web app, Compose Android app, SwiftUI iOS app; run on
   Chrome, Android emulator, iOS simulator.
7. **Contract scenarios** (SPEC §14) on all three platforms; **bench/RESULTS.md** against
   blueprint §14 budgets; CI workflows (Linux, macOS, Android).
8. **Adversarial reviews** of undra-runtime, undra-signals, undra-macros, undra-ffi with
   findings closed; reports in `.10x/reviews/`.

## 3. Known issues / open threads

* SPEC references ADR-014 (Kv is a foreign port) and ADR-017 (poisoning instead of CoW
  overlay); the ADR files were lost with the original `.10x/`. Recreate an ADR when its
  decision is next touched.
* Kotlin `NativeSmokeTests` (2 cases) skip until `undra-ffi` produces `libundra_core`.
* Swift XCTest needs full Xcode (`DEVELOPER_DIR` is set by `scripts/env.sh` when
  xcode-select still points at CommandLineTools).
* The desk-check-era `scripts/lb.sh` slice runner and `docs/AGENT_BRIEF.md` shell rules are
  VM artifacts; on this Mac run tools directly.

## 4. How to work (unchanged)

Read `CLAUDE.md` (constitution) and the SPEC sections for your piece first. One worktree
per piece; small `type(scope):` commits; every piece lands whole (tests, docs on every
`pub` item, `clippy -D warnings` clean, bench if the boundary is touched). Adversarial
review before merge; then `cargo test --workspace`, TS suite (`npm test` in
`runtimes/ts/@undra/runtime`), Kotlin suite (`runtimes/kotlin/undra-runtime/scripts/
test-local.sh`), Swift (`swift test` in `runtimes/swift/UndraRuntime`) all green before the
next piece starts. Update `.10x/status.md` + `.10x/handoff.md` when a piece lands.

## 5. Definition of done for v1

1. All twelve crates real, documented, clippy-clean; `cargo test --workspace` green.
2. Three platform runtimes green in their own suites **and** against the real core:
   Kotlin over JNI, TS over wasm (and remote), Swift over the C ABI (XCFramework).
3. `undra-cli` can init a project, build and package for all three platforms, and run
   `undra dev` with a live remote core.
4. The playground app runs on Chrome, an Android emulator and an iOS simulator from one
   Rust core, exercising todos, counter, 10k keyed list and a query.
5. SPEC §14 contract scenarios pass on all three platforms.
6. `bench/RESULTS.md` records the §14/blueprint budgets with real numbers; regressions
   fail CI.
7. CI workflows cover Linux, macOS and Android; Miri/ASan on `undra-ffi`.
8. Adversarial review reports for undra-runtime, undra-signals, undra-macros and undra-ffi in
   `.10x/reviews/` with findings closed.
