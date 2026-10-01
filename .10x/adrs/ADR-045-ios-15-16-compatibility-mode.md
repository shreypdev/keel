# ADR-045: an iOS 15/16 compatibility mode: generated stores as `ObservableObject`, chosen by the deployment target

Status: **Proposed** (2026-10-01, `wt/boundary-adrs`; Amendment B "iOS floor", catalogue M-9 and finding 5).
Touches SPEC 0 (the toolchain baseline: iOS 15 / macOS 12 as the Swift floor), 10.1 (a second generated store
shape and one type mapping), 17.3 (a few runtime signatures gain availability annotations) and the CLI's
`undra.toml`; `undra-bindgen` (a Swift generator option), `undra-cli` (passing the deployment target), the
Swift runtime. **No wire, schema, C ABI or Kotlin/TypeScript change.** Constitution R3 (a second generated
shape must pass native review on its own terms) and R11 (a generated public shape).

## Context

* SPEC 0: "Swift 6.0 / iOS 17+"; `runtimes/swift/UndraRuntime/Package.swift` declares `.iOS(.v17), .macOS(.v14)`;
  `undra.toml`'s `[ios] deployment_target` defaults to `"17.0"` (`crates/undra-cli/src/config.rs:108-124`) and is
  passed to cargo as `IPHONEOS_DEPLOYMENT_TARGET` and to the Xcode template (`crates/undra-cli/src/builds/ios.rs:98-101`,
  `commands/init.rs:205`).
* The reason is one macro: generated stores are `@MainActor @Observable` classes
  (`crates/undra-bindgen/src/swift.rs:859`; golden `crates/undra-bindgen/tests/golden/stores/swift/.../Stores.swift`),
  and Observation is iOS 17.0 / macOS 14.0 (catalogue source X4). The runtime itself does not import
  Observation.
* The catalogue (`.10x/specs/2026-10-01-competitive-limitations.md`): "KMP, React Native and Flutter all reach
  iOS 15" (finding 5: "the same window applies"); KMP-B7 "Kotlin/Native's default minimum is iOS 15.0"; RN-B5
  "iOS 15.1"; matrix row 24 "Supports iOS 15 and 16": KMP, RN, Flutter yes, Undra no. A team whose app supports
  iOS 15 or 16 today cannot adopt Undra without raising its floor first.

### What actually breaks below 17 (probe, this ADR; a copy of the runtime in the session scratch directory)

Built with `swift build` on the macOS host at the macOS deployment target that has the same API availability as
the iOS one (Observation: iOS 17 / macOS 14; `Duration`: iOS 16 / macOS 13); the typed-throws check was
type-checked for the iOS 15 simulator target directly.

| Floor | `UndraRuntime` | Generated stores (golden `stores`) |
|---|---|---|
| macOS 13 / iOS 16 | **builds unchanged** | `@Observable` fails: "'Observable()' / 'ObservationRegistrar' / 'ObservationTracked()' is only available in macOS 14.0 or newer", for every store and every property |
| macOS 13 / iOS 16, stores rewritten as `@MainActor final class …: UndraStore, ObservableObject` with `@Published public private(set) var` | — | **builds, Swift 6 language mode, no warnings** |
| macOS 12 / iOS 15 | fails; every error is `Swift.Duration` or `ContinuousClock` (iOS 16 / macOS 13 APIs) in three files: `Wire/UndraTypes.swift` (`UndraDuration` wraps `Duration`), `Core/MirrorStats.swift` (`DrainStats.duration: Duration`), `Core/Mirror.swift` (drain timing) | also the wire `Duration` mapping (`elapsed: Duration`) |

Typed `throws(E)` in an async protocol requirement (port requirements, ADR-032), `AsyncThrowingStream` and a
`@MainActor` `ObservableObject` with `@Published private(set)` type-check for `arm64-apple-ios15.0-simulator` and
`arm64-apple-macos12` with Swift 6 (same probe). Xcode 26.6's iOS SDK accepts deployment targets down to 12.0
(`SDKSettings.json`, `MinimumDeploymentTarget`).

## Decision

1. **Two store shapes, one mirror.** The Swift generator gains `swift_observation: Observation |
   ObservableObject`:
   * `Observation` (today): `@MainActor @Observable public final class Todos: UndraStore`, properties
     `public private(set) var`.
   * `ObservableObject`: `@MainActor public final class Todos: UndraStore, ObservableObject`, properties
     `@Published public private(set) var`, `import Combine` instead of `import Observation`. Everything else —
     `init(adopting:core:)`, constructors, commands, `apply(signal:op:reader:)`, the mirror registration, ADR-031
     delivery, ADR-032 errors — is byte-for-byte the same generated code. The runtime's `UndraStore` base class is
     unchanged; the mirror underneath is the same.
2. **Chosen by the deployment target, overridable.** `undra bindgen` reads `[ios] deployment_target` (and the
   macOS target when C5 adds one): below 17.0 (macOS 14.0) → `ObservableObject`, else `Observation`.
   `[bindings] swift_observation = "observation" | "observable-object"` (and `--swift-observation` on the
   command line) overrides it; asking for `observation` with a target below 17 is a CLI error naming both
   values. The generated file header records the mode, and `undra bindgen --check` treats a mode change as stale
   bindings.
3. **iOS 15 also needs a `Duration` without `Swift.Duration`.**
   * The runtime's floor becomes `.iOS(.v15), .macOS(.v12)`. `UndraDuration` stores `nanoseconds: Int64` (exact,
     the wire value) and offers `timeInterval: TimeInterval` and, `@available(iOS 16, macOS 13, *)`,
     `duration: Duration` and `init(_ duration: Duration)`; `extension Duration: UndraCodec` becomes
     `@available(iOS 16, macOS 13, *)`. `DrainStats` gains `durationNanoseconds: Int64`, and its `duration:
     Duration` becomes an iOS 16 accessor; the mirror times drains with `DispatchTime.now().uptimeNanoseconds`.
   * Generated code maps the wire `Duration` to **`Duration` when the target is 16 or later and `UndraDuration`
     below** (exact either way; a `TimeInterval` would lose nanoseconds and break `Hashable` round trips).
4. **What degrades, said plainly.**
   * **Granularity.** With Observation a view re-renders when a property it *read* changes. With
     `ObservableObject` every view that observes a store re-renders when *any* published property of that
     store changes; within one drain several property sets send several `objectWillChange` events that SwiftUI
     coalesces into one update per run-loop turn. Big stores feeding many views should be split, or views
     should observe smaller stores (ADR-040's child stores help).
   * **SwiftUI usage changes** (the docs show both side by side):

     | | iOS 17+ (`Observation`) | iOS 15/16 (`ObservableObject`) |
     |---|---|---|
     | owning view | `@State private var todos = try! Todos()` | `@StateObject private var todos = try! Todos()` |
     | child view | `let todos: Todos` | `@ObservedObject var todos: Todos` |
     | environment | `.environment(todos)` / `@Environment(Todos.self)` | `.environmentObject(todos)` / `@EnvironmentObject` |
     | outside SwiftUI | `withObservationTracking` | `todos.$visible.sink { … }` (Combine, per property) |

     The `$property` publishers are a gain for UIKit and Combine code on every floor.
   * Runtime types that generated stores expose (ADR-043's `UndraLazyList`) ship in both forms
     (`@available(iOS 17, *) @Observable` and an `ObservableObject` twin); the generator picks one by mode.
5. **Raising the floor later** is a regeneration plus mechanical view edits (the table above); the cookbook
   has the migration. No core change, no wire change.
6. **What does not change.** Kotlin and TypeScript; the C ABI; the XCFramework (`IPHONEOS_DEPLOYMENT_TARGET`
   is already passed to cargo); Swift 6 language mode and strict concurrency; ADR-041's `@MainActor` callback
   protocols and ADR-040's identity map, which need nothing newer than iOS 15.

## Alternatives considered

* **Hand-rolled observation** (the generated class keeps an `ObservationRegistrar` on 17+ and a Combine
  subject below, behind `#available`). One class cannot be `@Observable` conditionally; stored properties
  cannot have availability-limited types; and a view that must also run on 15 uses `@StateObject` anyway,
  which invalidates on `objectWillChange` whatever else the class does. Complexity without a benefit a view
  can use.
* **Two classes per store (`Todos` and `TodosObject`).** Doubles the generated surface and every API that
  returns a store (ADR-040) would have to choose; the deployment target already decides.
* **Back-port Observation** (Perception-style libraries). A third-party dependency in generated code, and a
  floor-specific API shape anyway.
* **Raise nobody's floor; say iOS 17 is the price.** The catalogue's finding is that this blocks adoption for
  teams whose apps support 15 and 16; the probe shows the cost is one generator option and three runtime
  files.
* **Floor at iOS 16 only.** Saves the `Duration` work, but RN's floor is 15.1, KMP's and Flutter's 15.0.

## Consequences

* Apps that support iOS 15 or 16 (macOS 12 or 13) adopt Undra; catalogue row 24 moves to "yes".
* The Swift runtime package's floor drops to iOS 15 / macOS 12; three files gain availability annotations;
  `DrainStats` gains a field.
* There are two Swift goldens for the store-bearing cases; bindgen's Swift tests run both.
* The playground's iOS app stays on 17 (it dogfoods Observation); a small compatibility sample proves 15.

## Risks

* **Behavioural drift between the modes** is limited to how SwiftUI is notified; the apply code is shared. The
  Swift contract suite runs once in each mode (decision "Implementation brief" 5).
* **Simulator coverage.** This machine has only the iOS 26.5 simulator runtime; whether the CI Xcode can still
  *run* iOS 15/16 simulator runtimes must be confirmed when the job is built (it can *build* for them).
  Without one, the floor is proven by compilation plus one manual run on a device (E1's device pass).

## Implementation brief

1. `crates/undra-bindgen`: `Generator::swift_observation`; `swift.rs` emits the `ObservableObject` shape
   (header comment with the mode, `import Combine`, `@Published`), the `Duration`/`UndraDuration` mapping by a
   `swift_min_ios` field, the runtime lazy-list type by mode. Golden trees `swift/` and
   `swift-observable-object/` for `stores`, `queries` and `full`; both type-checked by the golden test at their
   floors (`-target arm64-apple-ios15.0-simulator` and `...-ios17.0-simulator`).
2. `crates/undra-cli`: read `[ios] deployment_target` into the generator; `[bindings] swift_observation` and
   `--swift-observation`; the conflict error; `--check` covers the mode; `undra init --ios-deployment-target 15.0`
   generates an app template that uses `@StateObject` (two template variants of the bootstrap and sample view).
3. `runtimes/swift/UndraRuntime`: platforms `.iOS(.v15), .macOS(.v12)`; `UndraDuration` stores nanoseconds;
   availability on `Duration` APIs; `DrainStats.durationNanoseconds`; drain timing without `ContinuousClock`;
   README and DocC notes per mode.
4. CI: a job that builds the runtime and both golden modes for iOS 15.0 and 16.0 simulator targets and macOS 12,
   and runs `swift test` (host) as today; when a 15/16 simulator runtime is installable, the Swift contract suite
   runs there in `ObservableObject` mode.
5. Contract: the Swift contract suite gains a build variant generated in `ObservableObject` mode and runs the
   whole grid against it (no new scenario: the behaviour is the same by construction, and this proves it).
6. A compatibility sample (`examples/ios15-sample`, built in CI, not part of the playground) with a store, a
   query handle and a list, at a 15.0 deployment target.
7. Docs: SPEC 0, 10.1, 17.3; the Swift runtime README; the cookbook's "supporting iOS 15 and 16" page (the
   table of decision 4, the migration to Observation).

## Dependencies

None. ADR-043's `UndraLazyList` follows decision 4's two-form rule when it lands; whichever lands second adds
the twin.
