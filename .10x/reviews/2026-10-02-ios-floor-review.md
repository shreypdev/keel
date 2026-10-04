# iOS 15 / 16 compatibility mode (ADR-045) - adversarial review

**Date:** 2026-10-02 · **Reviewer:** adversarial (`docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/ios-floor` at `7e7c6b9` (`main` `3fc8b7f`
is an ancestor) · **Read:** ADR-045 with its deviations, `.10x/decisions/sde/ios-floor.md`, `docs/IOS_15_16.md`, SPEC 17.3 and 10.1, and the
diff (`undra-bindgen` `swift.rs`/`validate.rs`/the `swift-observable-object` goldens, `undra-cli` `config.rs`/`bindgen.rs`/`templates/ios-floor`, the five
runtime files, `examples/ios15-sample`, `scripts/ios-floor.sh`, `contract-tests/swift/run.sh --floor`, `ci.yml`) · **Scope:** the five surfaces of the
brief and nothing else · **Fixes:** `fix(ios-floor): review fixes - …` (one commit), then this record.

## Verdict

**Sound with fixes; merge.** No High. The shape holds where it matters: the apply code, the mirror registration, the commands and `onError` are
the `@Observable` shape character for character (a generator test compares the two after undoing the four differences), the 24-scenario Swift
contract grid passes at the floor with the same scenario lines as the default mode, the stores publish once per merged apply, and the floor
compiles for the 15.0 and 16.0 simulator targets (`minos 15.0` in the objects, `libswiftObservation` weak-linked in the iOS 15 app). Two Mediums,
both fixed with a test that fails without the fix: the `@Observable` connection (`core.connection`) could miss the final state when the app
releases the core from `onConnectionChange` (M1), and the `undra.toml` diagnostics for the floor settings lost their specific *why* (M2). One
required number the docs did not carry: `@Published` copies a list once per applied patch, **48 us at 10,000 rows against 0.2 us** (M3, stated
now). Three Lows. Open items below, none blocking.

## Findings

**M1 - a view's `@Observable` connection depended on the core being alive (fixed; `UndraCore.swift:138,392`).** `setConnectionState` and the
placeholder branch of `init` hopped to the main queue with `[weak self]` and looked the Observation twin up through `self`. `onConnectionChange`
runs *before* the hop, and the documented reaction to `.closed(.sessionLost)` is to load a new core and drop the old one, so the old core can be gone
when the hop runs: the `ObservableObject` connection (captured strongly) heard `.closed`, the `@Observable` one stayed on its last state. The default
mode had no such dependency before this branch (the hop captured the connection itself). Fix: both observables are captured, not `self`. Test:
`ReconnectCoreTests.testAConnectionAViewHoldsHearsTheFinalStateEvenWhenTheCoreIsReleasedAtOnce` (the fake transport gains
`releasesInboundOnShutdown`, as the in-process transport forgets the core on shutdown); it timed out at 3.1 s without the fix.

**M2 - the floor's R8 diagnostics lost their why in `undra.toml` (fixed; `config.rs:389,394,680`).** `deployment_target = "14.0"` and `observation` with
a floor below 17 were mapped through `CliError::bad_config(file, what, fix)`, whose `why` is the general "the file drives the build…". The specific why
(Combine/ObservableObject; "Observation, which is iOS 17.0 and later") only survived on the command line. `in_file` keeps the why. Test:
`every_ios_floor_mistake_teaches_what_why_fix_and_where_to_read_more` (14.0, `sixteen`, 16.4 with `observation`; 15, 15.0, 15.0.1, 16.4, 17.0, 26.0 and
`observation` on 17.0 are accepted; the command line's `C0009`); it failed on the generic why without the fix.

**M3 - the `@Published` list cost was unstated (fixed in docs; measured below).** ADR-045 decision 4 names granularity only. `@Published` has no
in-place accessor, so `try applyPatch(ops, to: &self.visible)` copies the array once per applied patch. Now in SPEC 10.1, `docs/IOS_15_16.md`, the
cookbook page and an ADR-045 addendum.

**L1 - `ios_major` took `+15` and four-part versions (fixed; `config.rs:695`).** `u32::from_str` accepts a leading `+`; the value would have reached
`IPHONEOS_DEPLOYMENT_TARGET`. It takes digits, at most three parts. Test in `a_deployment_target_the_runtime_cannot_run_on_is_refused`.

**L2 - a build marker was tracked (fixed).** `examples/ios15-sample/build/ios/.undra-configuration-Debug` is what the Xcode phase writes; the sample had
no `.gitignore` (the other examples do). Untracked; `.gitignore` added.

**L3 - the floor's main-queue twin test was only indirect (closed).** `testTheConnectionObjectIsTheObservableObjectTwin…` polled each observable once.
`testBothConnectionObservablesFollowEveryState…` takes both through connecting, connected, reconnecting, connected, closed and requires them to agree at
each step, that `core.connection` is one object, and that an Observation tracker on it fires.

## The attacks and what they found

**1. R3 on the `ObservableObject` output.** Read `stores` and `full` by eye and the `stores` diff against the default: `@MainActor` on its own line,
`final class Todos: UndraStore, ObservableObject, @unchecked Sendable`, `@Published public private(set) var` per signal, `import Combine` in place of
`import Observation`, a second header line naming the mode: what a Swift engineer writes (`$visible` is a public Combine publisher). The query handles
differ in the same four places and no others. The generator test `the_observable_object_mode_changes_only_how_a_store_is_observed` compares the whole
`Stores.swift` after rewriting the mode's four differences back to the other spelling, so the constructors, `init(adopting:core:)` (the mirror
registration, `noCoalesce: [0]`), the commands (`self.core.report(error, operation:)`, non-throwing) and `apply` are compared: right thing. `@unchecked
Sendable` has its reason in the generator (`swift.rs:1196`) but not in the output, in either mode (open item). **Keyed list cost**, measured: scratch
package over the runtime's `applyPatch`, release build, Apple M5 Pro, macOS 26.5, 32-byte rows with a heap string, median of 60 to 300 applies, a sink
on `objectWillChange`:

| rows | ops | Observation (`@Observable`) | `@Published` + `objectWillChange` | `@Published`, array swapped out and back |
|---|---|---|---|---|
| 1,000 | 1 | 0.2 us | 5.0 us | 0.8 us |
| 10,000 | 1 | 0.2 us | **48 us** | 1.0 us |
| 10,000 | 100 | 8.1 us | 56 us | 9.2 us |
| 100,000 | 1 | 0.2 us | 0.5 ms | 0.9 us |

O(n) in the list (about 5 ns a row), once per applied patch not per op (100 ops cost what 1 does plus 8 us), which the mirror's merging keeps to once per
frame per signal. The swap-out trick removes the copy but publishes `[]` to `$rows` subscribers in between, so it is not a fix. Accepted as the known
cost, stated with the number.

**2. Semantics equivalence.** `contract-tests/swift/run.sh` (default) and `--floor`: 24/24 each, the `SCENARIO` lines identical, the one skipped test in
both is `MigrationBuildB` (it runs in its own process by design). The floor run's bindings were verified to be `@Published` (153 properties, no
`@Observable`). S18 counts mirror applies, not publishes, so it proved nothing about `@Published`: under `UNDRA_FLOOR` it now counts `objectWillChange` on
the generated `Stress` store (1 for the 1,000-change-set firehose burst, 10 for ten `no_coalesce` progress entries; both hold). Unit test
`testAHundredMergedEntriesPublishOncePerSignalOnAnObservableObjectStore`: 100 change-sets, each an insert patch of a `@Published` list and a full value of
a scalar, one drain: `entriesApplied` 2, `objectWillChange` 2, one value on `$rows` (with all 100 inserts), one on `$total`.

**3. The runtime twins and the floor types.** Negative duration from the wire is `WireError.negativeDuration` (typed; tests in `TypedValueTests` and
`MalformedInputTests`), saturating conversions use overflow-reporting arithmetic. The drain is timed with `DispatchTime` (`uptimeNanoseconds`:
monotonic, excludes system sleep, which is right for a main-actor drain), `Int64(clamping:)`. `UndraConnection`/`UndraConnectionObject`: M1/L3.
`sqlite3_changes` returns a C `int`: over 2,147,483,647 changed rows the count is undefined; `sqlite3_changes64` is SQLite 3.37, iOS 15.4, so it cannot
be linked at the floor, and a statement that size is not one a phone runs: documented (`docs/IOS_15_16.md`, cookbook), not guarded. Gates: the runtime
builds for the iOS 15.0 simulator and macOS 12 with no warning (`minos 15.0` confirmed with `vtool` on a rebuilt object), and the iOS 15.0 sample's
`libswiftObservation.dylib` is a weak load command, so an iOS 15 launch does not need it. **No silent fallback on a newer device:** a probe executable
(`scripts/ios-floor-probe`, run by `scripts/ios-floor.sh probe`, in CI) built for iOS 15.0 and for 17.0 (`minos` confirmed) and run with `simctl spawn` on
the iOS 26.5 simulator reports `observation=available same-object=true tracker-fired=true agree-with-connectionObject=true` for both. (The package test
bundle cannot be built for the iOS simulator at all: `RealtimeAdapterTests` uses `Process`; that is why the probe exists.)

**4. CLI and diagnostics (R8).** The brief's inputs: `14.0` (C0002/C0009, "below the lowest iOS Undra supports, 15.0"), `15`, `15.0.1` (accepted), `17.0` with
`observation` (accepted), `16.4` with `observation` (names both settings, fix names `observable-object` and 17.0); each has code, what, why, fix and the
docs link, once through `undra.toml` and once through the flags (tests in `config.rs` and `bindgen_schema.rs`; M2). `undra init --ios-deployment-target 15.0`
emits the floor template (existing test `an_ios_15_project_gets_observable_object_stores_and_views_that_observe_them`, plus the Xcode build of such an app in
`platforms.rs`). `undra bindgen --check` detects the switch: new test `a_swift_mode_or_floor_switch_without_regeneration_is_stale_bindings` (17 to
`--swift-observation observable-object`, to floor 15, to floor 16 each stale and naming `Stores.swift`; 16 also `Package.swift`; regenerating makes it current and the
other stale; back to the default current). E0051 for `objectWillChange`: golden present (what, why, fix, docs), only in the mode (CLI test
`a_store_member_named_object_will_change_is_refused_at_the_floor_only`), `site/docs/errors.html` equals a rebuild (`build-errors: 46 codes … up to date`).

**5. The matrix:** below.

## Counts (the tree at this commit, one macOS 26.5 / Xcode 26.x / Swift 6.3 machine)

| Check | Result |
|---|---|
| `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` (with `tsc` from `npm ci` in this worktree) | 3,078 pass, 0 fail, 16 ignored; after the last edit (`ios_major`) `undra-cli --lib` 311 and `--test bindgen_schema` 9 re-run |
| `swift test` (`runtimes/swift/UndraRuntime`) | 674 pass (671 + 3 new) |
| Swift contract grid, default mode (`run.sh`, graded by `check.sh swift`) | 24/24 |
| Swift contract grid, floor (`run.sh --floor`, in `scripts/ios-floor.sh contract`) | 24/24, `TestKitTests` 4/4; scenario lines identical to the default |
| `undra bindgen --check --docs` playground (`0xb5b7b1dc29182a9d`), Fieldbook (`0x7bfb0229a00c20ed`), cookbook (`0x88127919dcf53b11`), two-cores a and b | all up to date |
| `undra bindgen --check` `examples/ios15-sample` (`0x5eb5bc24931c586b`) | up to date; with `--docs` it differs by design: an `undra init` project's bindings carry no doc comments, and its README says `--check` |
| `scripts/ios-floor.sh runtime golden sample apps contract probe simulator` | rc 0: runtime for iOS 15.0 simulator and macOS 12; every golden case, `ObservableObject` mode, iOS 15.0 and 16.0 (`typecheck_swift` ios_15, ios_16: 2 pass); `xcodebuild` sample (`MinimumOSVersion 15.0`); playground and Fieldbook bindings at 15; the probe twice; no iOS 15/16 runtime installed, said so |
| `xcodebuild` playground (iOS 17.0, Debug, generic simulator) | builds |
| `scripts/wasm-size.sh` | hello-wasm 116,900 B gzipped (gate 120,000), hello-runtime-js 25,996 (gate 26,000): unaffected |
| site `build-all.mjs` then `check-links.mjs --words` | no file changes after the build; links clean, landing 342 words |
| `ci.yml` | parses (Ruby `YAML.load_file`); job `ios-floor` runs `scripts/ios-floor.sh` steps only, all of which ran above; the probe was added to its last step |

## Open items (none blocking)

* **No iOS 15 or 16 runtime was run** (none installed, none downloaded). Compilation, `minos`, the weak Observation load command and the 26.5 runs are the proof;
  that the iOS 15.x system Swift runtime resolves everything the Swift 6 runtime package references (typed throws in async requirements, `MainActor.assumeIsolated`)
  rests on the compiler's availability checking until `scripts/ios-floor.sh simulator` or a device pass (E1) runs it.
* **`@unchecked Sendable` has no comment in the generated code** (either mode): the reason is in the generator. One generated line in both modes would churn every
  generated tree (the default's byte-identity is a stated property of this piece), so it should be one sweep, not part of it.
* **`UndraDuration` has no seconds or milliseconds constructor**; an iOS 15 app writes `UndraDuration(nanoseconds: 1_500_000_000)`. Additive runtime API, no ADR.
* **The list copy** has a real remedy only in the generated shape (a private backing array and a manual `objectWillChange.send()`, losing `$property` for lists):
  an ADR if lists of 100,000 rows turn out to matter on iOS 15.
* **A wording nit:** `--swift-observation observation --ios-deployment-target 16.4` says "`deployment_target = "16.4"` in [ios]" though the value came from the flag.
* `typecheck_swift`'s floor variants compile but do not execute the generated code; the floor contract grid is what does.
