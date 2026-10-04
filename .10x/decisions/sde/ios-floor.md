# SDE: ios-floor - the iOS 15 / 16 compatibility mode (ADR-045), 2026-10-01

Worktree `wt/ios-floor`, from `main` `8cbfc0f`. Sources of truth: ADR-045 (now Accepted; its deviations section is the dated
record of what differs from the proposal), SPEC §10.1/§12/§17.3, ADR-024/032/044/051. No wire, schema, C ABI, Kotlin or TypeScript
change: the default Swift output (`@Observable`, iOS 17) is byte-identical to before (no golden, example or generated package moved);
`undra bindgen --check` is clean for the playground, Fieldbook, cookbook and two-cores. Nothing under `.10x/status.md` or
`handoff.md`. The boundary is not touched, so no benchmark (R4: the rule asks for one only if the boundary is).

## The mechanism

`Generator::swift_observation` (`SwiftObservation::{Observation, ObservableObject}`) and `swift_min_ios` (the floor's major).
`ObservableObject` mode emits `@MainActor public final class Todos: UndraStore, ObservableObject, @unchecked Sendable` with
`@Published public private(set) var` per signal and `import Combine`; every other line of a store, a query handle, the apply function
and the mirror registration is the `@Observable` shape's, character for character (a generator test compares them after undoing the four
differences). The wire `Duration` is `Duration` from a floor of 16 and `UndraDuration` below. Non-default mode adds a second header line
(`// Swift mode: observable-object …`), so a mode switch is a diff for `--check`.

`undra.toml`: `[ios] deployment_target` (validated: a version, 15.0 or later) decides the mode (below 17.0 `observable-object`);
`[bindings] swift_observation` and `undra bindgen --swift-observation` override; `observation` below 17.0 is a CLI error naming both
settings; `undra bindgen --ios-deployment-target` generates for another floor for one run; `undra init --ios-deployment-target 15.0`
writes the floor into `undra.toml`, the Xcode project, the generated package (`.iOS(.v15), .macOS(.v12)`) and three template views
(`ios-floor/`: `@ObservedObject`, `NavigationView`, `UndraConnectionObject`; the app's `@State` owns the stores, the screens observe them).
`undra upgrade` prints a migration note.

Runtime (`runtimes/swift/UndraRuntime`): platforms `.iOS(.v15), .macOS(.v12)`; five files changed: `Wire/UndraTypes.swift`
(`UndraDuration` over `Int64` nanoseconds, `.zero`, `timeInterval`, iOS 16 `Duration` conversions and `extension Duration: UndraCodec`),
`Core/MirrorStats.swift` + `Core/Mirror.swift` (`DrainStats.durationNanoseconds`, `DispatchTime`), `Core/Connection.swift` + `Core/UndraCore.swift`
(`UndraConnection` iOS 17, `UndraConnectionObject` twin, `core.connectionObject`), and `UndraTestKit/PreviewCore.swift` (no `ContinuousClock`). The
package's test target is built by SwiftPM at macOS 14 whatever the package floor says, so the tests keep using `Duration` and `@Observable`.

## Diagnostics (R8)

* CLI: `observation` with a floor below 17 (what, why, fix naming `swift_observation` and `deployment_target`); a deployment target below 15.0 or not a
  version; an unknown `swift_observation` value; `--swift-observation` / `--ios-deployment-target` the same.
* Bindgen: E0051 for a store member named `objectWillChange` in `observable-object` mode only (`Generator::swift_floor_errors`, a case in
  `tests/golden/diagnostics/E0051.txt`, which the site's error-codes page shows). It is the only schema feature that exists only at the floor.

## Verification (every step on this machine, macOS 26, Xcode 26.x, Swift 6.3)

| Check | Result |
|---|---|
| `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` | 3,073 pass, 16 ignored, 0 fail after merging `main` (ports). The first full runs had 21 environmental failures, none in this change: 19 TypeScript-toolchain tests (`run_ts`, `typecheck_ts`: this worktree has no `runtimes/ts/@undra/runtime/node_modules`; they pass with the main checkout's linked in) and 2 to 3 different `dev_reload` tests per run that time out when other agents' builds saturate the machine (all 9 pass with `--test-threads=1`, twice) |
| `cargo test -p undra-bindgen --test typecheck_swift` (default mode on macOS 14; ObservableObject mode for the iOS 15.0 and 16.0 simulators, all 10 golden cases each; `-target arm64-apple-ios15.0-simulator` confirmed with `-v`) | 4 pass |
| golden: `swift-observable-object/` for `stores`, `queries`, `full`; generator tests (8 new); E0051 case | pass |
| `swift test` in `runtimes/swift/UndraRuntime` | 671 pass after the ports merge (584 before it; 3 new: the `@Published` store through the mirror, `UndraDuration` on its own, `connectionObject`) |
| `bash contract-tests/run-all.sh swift` (default mode) | 24/24 pass (all the Swift column's scenarios, after the ports merge) |
| `bash contract-tests/swift/run.sh --floor` (the whole Swift column against playground + two-cores bindings generated for iOS 15: `ObservableObject` stores, `UndraDuration`; then the testing kit) | 24/24 pass (21/21 before the ports merge), `TestKitTests` pass |
| `undra bindgen --check --docs` playground, Fieldbook, cookbook; `--check` two-cores a/b, ios15-sample | all up to date (playground `0xfa536b9ac6f06149`, Fieldbook `0x7bfb0229a00c20ed`, cookbook `0x88127919dcf53b11`, two-cores a and b, ios15-sample `0x5eb5bc24931c586b`; two-cores need `--docs`, as before) |
| `xcodebuild` playground and Fieldbook (target 17.0, Debug, generic simulator) | both build (Debug, generic iOS Simulator) |
| `xcodebuild` `examples/ios15-sample` (target 15.0; `minos 15.0`, `MinimumOSVersion 15.0`); `UNDRA_TEST_IOS_APP=1 cargo test -p undra-cli --test platforms an_ios_15_app_builds…` (an `undra init --ios-deployment-target 15.0` app) | both build |
| playground + Fieldbook bindings generated for iOS 15, `swift build` for the iOS 15.0 simulator (`scripts/ios-floor.sh apps`) | both build |
| the iOS-15.0-built sample installed and driven on the iOS 26.5 simulator (store, keyed list and query handle through `@Published`: typed a todo, Add, "1 left"; the three tips) | works |
| XCUITest tour of the playground on iPhone 17 Pro (`examples/playground/ios/smoke.sh`) | 5 pass, bench test skipped by design (`xcodebuild test`, the tour of `smoke.sh`; the script's own `simctl io screenshot` step cannot write into the worktree under this sandbox, so the tests ran directly with `TEST_RUNNER_PROOF_DIR` in a scratch directory) |
| `scripts/wasm-size.sh` | unaffected: hello-wasm 116,899 B gzipped (gate 120,000), hello-runtime-js 25,984 (gate 26,000) |
| site `build-all.mjs` (new cookbook page, docs nav, E0051 message, CLI page) and `check-links.mjs --words` | clean, landing 342 words |

## Deviations (also in ADR-045's dated section)

1. **No iOS 15 or 16 simulator runtime** is installed here (`simctl list runtimes`: iOS 26.5 only) and none was downloaded. The floor is proven by
   compilation for the 15.0/16.0 simulator targets, the sample's `minos`, a run of the 15.0-built app on the 26.5 runtime, and the contract
   grid in floor mode. Running on a 15/16 runtime remains a device/simulator pass (`scripts/ios-floor.sh simulator` does it where one exists).
2. **The sample, not the apps.** ADR-045 item 6 (a sample at 15.0) and the dispatch's "playground at the floor" were reconciled: the playground and
   Fieldbook *apps* stay on 17 (their screens use `NavigationStack`, `PhotosPicker`, `.environment(_)` and Observation itself); their
   *bindings* are generated for iOS 15 into a scratch directory and compiled for the iOS 15.0 simulator in CI, and the playground's Swift contract
   grid runs in floor mode; `examples/ios15-sample` is the app built at 15.0.
3. **The runtime did import Observation** (ADR-051's `UndraConnection`), so the ADR's three files were five; see ADR-045's deviations.
4. **Two floor-16 and mode-line choices**: the 16 combination has no golden tree (a generator test and a compile cover it); the mode line
   is only in the non-default mode (no churn).
5. `UndraLazyList` (ADR-043) has not landed; the twin rule is ready for it.
6. The reserved-entry list (`naming.rs`) gained `UndraConnectionObject`; found by the existing test over the runtime's public types.
7. Merging `main` (ports, ADR-047/048) brought `SQLiteDbAdapter` with `sqlite3_changes64` (SQLite 3.37: iOS 15.4 / macOS 12.3, above the floor): it uses `sqlite3_changes` now. The floor build is what caught it, which is the point of the `ios-floor` job.

## Where things are

`crates/undra-bindgen/{src/lib.rs,src/swift.rs,src/validate.rs,tests/{golden.rs,typecheck_swift.rs,generators.rs,diagnostics.rs,golden/*/swift-observable-object}}`,
`crates/undra-cli/{src/config.rs,src/bindgen.rs,src/commands/{bindgen,init}.rs,src/templates.rs,templates/ios-floor/,tests/platforms.rs}`,
`runtimes/swift/UndraRuntime/…`, `contract-tests/swift/{Package.swift,run.sh}`, `examples/ios15-sample/`, `scripts/ios-floor.sh`,
`.github/workflows/ci.yml` (job `ios-floor`), `docs/IOS_15_16.md`, `docs/SPEC.md`, `site/docs/cookbook/ios-15-16.html`,
`.10x/adrs/ADR-045…` (Accepted) and the note in ADR-024.
