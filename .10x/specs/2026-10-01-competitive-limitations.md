# Competitive limitations catalogue: KMP, UniFFI, Crux, React Native, Flutter, Capacitor and "write it three times"

**Date:** 2026-10-01 · **Status:** discovery document for the v1.1 / v1.2 program
(`.10x/specs/2026-10-01-v1x-default-choice-design.md` §1, including Amendment A on `main` at `b44cbad`,
which approved all six v1.2 bets) · **Author:** competitive analyst (opus), `wt/competitive` ·
**Reviewer at publication:** fable fact-check (as planned for H4).

The founder's instruction: *find all limitations of KMP and the other ways teams do this today, and fix
all of them in ours.* This catalogue is the evidence base for that checklist and for the post "Why Undra is
the default choice". It goes both ways. Every limitation of another tool has a primary source and a date.
Every limitation is mapped to Undra as **SOLVED**, **PLANNED** or **MISSING**. Every place another tool
still beats Undra is written down with a recommendation.

---

## 0. Summary

**The five findings that should change the plan** (expanded in §11 and in
`.10x/decisions/cto/competitive-limitations.md`):

1. **Android has no default platform adapters.** Swift and TypeScript ship Http, Kv, SecureStore, Fs and
   Connectivity adapters. On Android the runtime installs only Clock, Rng, Log and Timer, and the playground
   works around this with an in-memory Kv and an in-process demo server (E22). This means that on Android the
   data layer's persistence and offline queue only work after the team writes its own adapters. KMP teams get
   Ktor and DataStore ready-made. C4 calls this an "audit"; it should be a named Phase-1 piece
   (`UndraAndroid.load(context)` with the adapters `android-adapters/README.md` already specifies).
2. **The data layer stops short of what TanStack-class users expect.** It has no interval polling: SPEC §9
   lists it, but the code does not implement it. It has no paged or infinite queries: `Lazy<T>` is rejected in
   v1, but E3 ("lazy-list ergonomics") is scoped as if it existed. A queued offline mutation also loses its
   optimistic state and its invalidations when the app restarts. E3 needs an ADR and a re-scope, and the
   restart case belongs in A5.
3. **The boundary surface is narrower than UniFFI's and flutter_rust_bridge's.** Undra has no objects as
   parameters or returns (E0064), no host callbacks or listener objects as arguments (E0004), no generics
   (E0002) and no newtypes (E0007). C3 covers numerics, tuples and similar types, not these. They are
   runtime-model and wire changes that need ADRs. Rust teams moving from UniFFI, our first market, hit E0064 on
   day one.
4. **Running in production is unplanned.** No crash-symbol files are emitted, step-debugging from Swift or
   Kotlin into the core is undocumented, and nothing integrates with OS background execution
   (BGTaskScheduler / WorkManager). Every production team hits the first. Offline-first teams hit the third.
   None of the three is in Tracks A–H.
5. **Two shape decisions get expensive once anything is published.** The first is per-core ABI namespacing
   (today only one Undra library can exist per app; KMP documents the same pain [K7], and UniFFI's
   crate-namespaced symbols appear to avoid it, which we have not verified in a two-library app). The second
   is the iOS 17 floor (KMP, React Native and Flutter all reach iOS 15). The plan already uses the argument
   "change the wire while nothing is published" for A4. The same window applies to these two. Decide both by
   ADR in Phase 1.

**Counts.** Each alternative's limitations are mapped to Undra. N/A means out of scope by an explicit product
decision.

| Alternative | Limitations catalogued | SOLVED | PLANNED | MISSING | N/A |
|---|---|---|---|---|---|
| Kotlin Multiplatform (+ CMP) | 18 | 11 | 2 | 4 | 1 |
| UniFFI | 15 | 13 | 2 | 0 | 0 |
| Crux | 9 | 5 | 4 | 0 | 0 |
| React Native (New Architecture) | 9 | 4 | 4 | 1 | 0 |
| Flutter | 9 | 4 | 4 | 1 | 0 |
| Capacitor / Ionic | 3 | 2 | 1 | 0 | 0 |
| Write the logic three times | 5 | 4 | 1 | 0 | 0 |
| **Total** | **68** | **43** | **18** | **6** | **1** |

The low MISSING count does not mean Undra is ahead. Most of what Undra lacks is not a *limitation* of an
alternative. It is something an alternative *does better*. The "where it still beats Undra" tables in §3–§7
list 30 such items, and §8–§9 add notes on Capacitor and the status quo. §11 turns them into 16 proposals
(M-1…M-16) and two explicit non-goals (M-17, M-18). The matrix in §10 is the honest single view.

**Three places the alternatives clearly beat Undra today:**

1. **Ecosystem, backing and production record.** KMP has JetBrains plus Google's stated support and
   KMP-ready Jetpack libraries (Room, DataStore, Paging, ViewModel). React Native has Meta and Expo. Flutter
   has Google. UniFFI ships in Firefox. Undra is a v1.0 from one team, with no production users.
2. **Developer tooling.** Flutter and React Native ship first-party DevTools (inspector, profiler, network,
   memory). JetBrains' KMP plugin does cross-language Swift/Kotlin navigation and debugging. Flutter and RN
   reload logic in place. Undra has log records, a live core on iOS and the web only, no reload that keeps
   state, and no documented debugger path.
3. **Breadth.** They offer shared UI when a product wants it (CMP, RN, Flutter). They reach desktop on
   Windows and Linux, tvOS and watchOS, and iOS 15. They are more flexible at the boundary: KMP shares any
   Kotlin, UniFFI passes objects and callbacks and generates Python and Ruby, and flutter_rust_bridge takes
   "arbitrary" Rust types.

---

## 1. Method and rules

* **Dates.** Every statement about another tool is *as of 1 October 2026*, the day the sources were
  fetched. Each row gives the source and, where the page shows one, the page's own last-modified date. Pages
  move, especially for the pre-1.0 tools (UniFFI, Crux), and Swift export is Alpha. Re-check before
  publishing.
* **Primary sources only.** Sources are official documentation, READMEs, release notes, source templates and
  issue trackers. Two third-party articles are used and labelled 3P: Touchlab, the maker of SKIE, and Photoroom's
  engineering blog on Crux.
* **No borrowed benchmarks.** The only numbers here are Undra's own (`bench/RESULTS.md`,
  `docs/HIGH_FREQUENCY.md`). Release posts and package descriptions contain other figures, such as React
  Native 0.82's Hermes V1 timings and BoltFFI's crate description. They are deliberately not repeated.
* **Basis codes** (the same as the claims ledger `.10x/reviews/2026-09-30-blog-claims.md`):

  | Code | Meaning |
  |---|---|
  | RAW | a raw file (README, CHANGELOG, source template) read verbatim |
  | DOC | an official documentation page, read with a fetch tool that returns a model-written extract; the quoted sentence should be confirmed on the page |
  | 3P | a third-party article |
  | REPO | this repository, `file:line` |
  | DER | **judgement or inference** from the rows it names. These are not facts, and the post must word them as opinion |

* **Strict mapping.**
  * **SOLVED** means the repository has the feature, with a file, a contract scenario or a test as evidence
    (§2). A design intention in `docs/blueprint.html` does not count.
  * **PLANNED** means a named piece of the v1.x program covers it.
  * **MISSING** means nothing covers it. §11 gives a proposal, a sketch of what solved looks like, and a size.
  * **N/A** means out of scope by an explicit product decision (shared UI: README "Not in v1", blueprint
    ADR-012).
* **Partial.** A row that is partly solved is marked by what remains. If the remainder is planned, the row
  is PLANNED; if not, it is MISSING.
* **What is not counted.** Maturity and churn (pre-1.0 breaking changes and the like) appear in §9 rather
  than the counts. Counting another tool's youth against it while Undra is younger would be self-serving.

---

## 2. Undra baseline: what the repository proves (the evidence the mappings cite)

| Id | Fact about Undra today | Evidence |
|---|---|---|
| E1 | Swift, Kotlin and TypeScript source is generated from a schema the macros extract. There is no Objective-C, and golden files lock the output | `docs/SPEC.md` §10; `crates/undra-bindgen/tests/golden/`; `examples/playground/generated/` |
| E2 | Calls are Swift `async throws`, Kotlin `suspend` and TS `Promise`. Streams are `AsyncThrowingStream` / `Flow` / `AsyncIterable`, with credit-based backpressure | SPEC §3.7, §10; contract S04, S07 |
| E3 | Enums and errors become native sum types: Swift enums with associated values, Kotlin sealed classes and interfaces, TS discriminated unions | SPEC §10.1–10.3; S02 |
| E4 | Every failure is a typed value. A Swift call throws its own `E`, `CancellationError` or `UndraCallError`, and a synchronous command reports through `onError`. A native panic is caught at the boundary (status 2). On wasm a panic traps, the call rejects, and the page restores from a snapshot | SPEC §5.6, §7, §10.1; `docs/SWIFT_ERRORS.md`; ADR-032; S05, S17 |
| E5 | Reads never cross the boundary. Each transaction produces one change-set per store, and keyed lists cross as recorded O(change) patches. `Computed<Vec<T>>` crosses whole | CLAUDE.md R5; SPEC §5.5, §3.8; ADR-027; S09–S11; `bench/RESULTS.md` Finding 1 |
| E6 | There is no tracing GC. Objects are generation-tagged handles released by `close()`, with `deinit` / `Cleaner` / `FinalizationRegistry` as a backstop | SPEC §1.2, §11 "Handle lifetime" |
| E7 | **One core per process** on native, and the C ABI symbols (`undra_*`) carry no per-core prefix | SPEC §6 (`undra_init` "idempotent per process"); `contract-tests/scenarios.md:24`; `crates/undra-cli/templates/shim/lib.rs` |
| E8 | `undra dev` serves the core over WebSocket and swaps it on rebuild; the app reloads to reconnect. Web and iOS can connect. **Android has no remote transport.** State is not preserved across a rebuild | `crates/undra-cli/src/commands/dev.rs:1-6`; `site/docs/cli.html:108` |
| E9 | Web: a wasm32 core plus `@undra/runtime`, which has no runtime dependencies. Modes are `wasm-main`, `wasm-worker` and `remote`. Adapters exist for React, Vue, Svelte and Solid. `useSignal` is built on `useSyncExternalStore` with a server snapshot. Runtime plus hello-world core is 85 KB gzipped | `runtimes/ts/@undra/runtime/package.json`; `src/react.ts:21-42`; README.md:50 |
| E10 | Packaging produces a Swift package with an XCFramework, a Gradle module with 16 KB-aligned `jniLibs`, and an npm package. There is no CocoaPods dependency | `site/docs/cli.html` (bindgen, build); `crates/undra-cli/src/builds/` |
| E11 | Data layer: queries (staleness, dedup, retry with jitter), mutations with optimistic updates and stamp-exact rollback, an idempotent offline queue persisted in `Kv`, and persisted queries. **Limits:** no interval refetch; after a restart, a queued mutation's optimistic writes and `invalidates` targets are gone; persisted entries written by another schema are dropped | SPEC §9; `crates/undra-query/src/lib.rs:35-36, 59, 65-67, 86-87`; SPEC §9 line 573; S12–S14 |
| E12 | Stores are `@MainActor @Observable` classes (Swift), `StateFlow`s (Kotlin) and `Signal`s with hooks (TS) | SPEC §10; S08 |
| E13 | Delivery is merged per drain, aligned to frames and bounded; `no_coalesce` opts out | SPEC §11.1; ADR-031; S18; `docs/HIGH_FREQUENCY.md` |
| E14 | Cancellation works from either side and drops the core's future | SPEC §5.2; S06 |
| E15 | A schema-hash gate at load fails with a typed error naming both hashes; `undra bindgen --check` catches stale bindings in CI | SPEC §2.3, §11; S16 |
| E16 | CLI: `init`, `bindgen`, `build`, `dev`, `doctor`, `adopt` | `site/docs/cli.html`; `crates/undra-cli/src/commands/` |
| E17 | Android uses JNI with `RegisterNatives` and direct `ByteBuffer`s (no JNA) | SPEC §6.1 |
| E18 | Undra has its own executor, core thread, blocking pool and `Timer` port. On wasm it is single-threaded and the same core code runs | SPEC §5.1, §5.2, §5.8 |
| E19 | `TestRuntime` plus deterministic fakes (FakeHttp, MemKv, MemSecureStore, MemFs, FakeClock, SeededRng, CaptureLog, scripted events) | SPEC §8; `crates/undra-ports/src/fakes/` |
| E20 | Licensed MIT OR Apache-2.0 | README.md |
| E21 | Measured numbers (ours, host-side): 49.8 ns synchronous call (core half), 6.31 µs keyed insert into 10,000 observed rows, 831 KB Android core per ABI, 85 KB gz web | `bench/RESULTS.md`; README.md:43-51 |
| E22 | Standard ports with default adapters. **Swift:** Http, Kv, SecureStore, Fs, Clock, Rng, Log, Timer, Connectivity; the app reports Lifecycle. **TS:** fetch, IndexedDB, WebCrypto, OPFS, `navigator.onLine`, Page Visibility. **Android:** only Clock, Rng, Log and Timer; the rest is listed under "What it will hold". The playground's Android app supplies an in-memory Kv and a demo HTTP server | `runtimes/swift/.../Adapters/PlatformDefaults.swift:1-30`; `runtimes/ts/@undra/runtime/src/adapters/`; `runtimes/kotlin/undra-runtime/android-adapters/README.md:21-47`; `examples/playground/android/app/src/main/kotlin/dev/undra/playground/UndraApp.kt` |
| E23 | The core runs off the UI thread (a core thread on native, a worker optionally on web). The main thread only applies merged change-sets | SPEC §5.1, §11.1 |
| E24 | UI is native: the playground screens are SwiftUI, Jetpack Compose and React | `examples/playground/{ios,android,web}`; README "Not in v1: … shared UI of any kind" |
| E25 | The type surface rejects generics (E0002), trait objects and closures (E0004), tuple and unit structs, so no newtypes (E0007), objects as fields, parameters or returns (E0064), `Lazy<T>` (E0001) and `Option<Option<T>>` (E0063) | SPEC §12, §4.3; `crates/undra-macros/src/impl_/check.rs:353` |
| E26 | Platform floors are Swift 6 / **iOS 17+** (Observation) and Android API 26+ | SPEC §0 line 24; `runtimes/swift/UndraRuntime/Package.swift` (iOS 17, macOS 14); Observation is iOS 17.0 [X4] |
| E27 | No build-system integration: the generated Xcode, Gradle and Vite projects do not run `undra build`, and `undra adopt` writes instructions but never edits project files | `crates/undra-cli/templates/{ios,android,web}`; `crates/undra-cli/src/commands/adopt.rs:1-7` |
| E28 | No symbol files are emitted for crash symbolication. Release builds strip, and the web build runs `--strip-debug` | `crates/undra-cli/src/builds/*.rs`; `builds/web.rs:136` |
| E29 | Devtools are a log stream only: a record per commit, per port call and per panic. There is no inspector | SPEC §5.10 |
| E30 | Desktop: the Kotlin `:runtime` is plain JVM (contract tests run on the JVM). The Swift package declares macOS 14, but the XCFramework has iOS device and simulator slices only. The CLI does not support Windows | `runtimes/kotlin/undra-runtime/settings.gradle.kts`; `crates/undra-cli/src/builds/ios.rs:31-39`; README.md:137 |
| E31 | Records cannot evolve across a schema-hash mismatch; that is a v2 feature | SPEC §3.1 (line 185) |

---

## 3. Kotlin Multiplatform (+ Compose Multiplatform where relevant)

**Snapshot (as of October 2026).** Kotlin 2.4.20 is the version JetBrains' compile-time guide names as
latest [K5]. KMP has been Stable since Kotlin 1.9.20 (November 2023) [K22]. Google states that KMP "is
officially supported by Google for sharing business logic between Android and iOS" and lists KMP-ready
Jetpack libraries [K12, updated 23 Sep 2026]. Compose Multiplatform is Stable on Android, iOS and desktop
and Beta on web [K11, 1 Oct 2026]. Swift export is Alpha [K1, 28 Aug 2026]. JetBrains' roadmap targets a
Swift export release that is stable in 2026 [K8].

| Id | Group | Limitation a team hits (fact unless marked DER) | Basis · source (page date) | Undra | Evidence / piece |
|---|---|---|---|---|---|
| KMP-1 | interop | By default Swift reaches shared Kotlin through a generated Objective-C header. The direct path, Swift export, is "currently in Alpha" ("breaking changes are expected"), "works only in projects that use direct integration", and erases generic type parameters to their upper bounds | DOC [K1] (28 Aug 2026), [K2] (12 Aug 2026) | SOLVED | E1 |
| KMP-2 | interop | Over the Objective-C path, `suspend` functions appear as completion handlers, or as Swift `async`, which JetBrains calls "highly experimental". `Flow` has no mapping, so teams add SKIE (which converts Flow to `AsyncSequence`) or KMP-NativeCoroutines | DOC [K2]; DOC [K16] | SOLVED | E2 |
| KMP-3a | interop | Without SKIE, Kotlin enums and sealed hierarchies are not exhaustive Swift enums. This is inferred from what SKIE adds ("Transparent Enums", "Sealed Hierarchies … exhaustive switching"). Touchlab lists exhaustive sealed matching among Swift export's remaining gaps | DER from [K16]; 3P [K17] (12 Jun 2026) | SOLVED | E3 |
| KMP-3b | interop | Without SKIE, default arguments are lost in Swift (SKIE "adds overloaded methods") | DER from [K16] | MISSING | Undra has no parameter defaults either (Rust has none); only record fields take `#[undra(default)]`. M-16 |
| KMP-4 | errors | "Other Kotlin exceptions reaching Swift/Objective-C are considered unhandled and cause program termination." Only `@Throws` exceptions become Swift errors | DOC [K2] | SOLVED | E4 |
| KMP-5 | perf | "When a Kotlin collection is passed to Swift … the Swift compiler copies the entire collection" | DOC [K2] | SOLVED | E5: reads are local and lists cross as O(change) patches. Honest cost: Undra decodes and copies once per *change* per observed signal and holds a mirror, so an observed keyed list exists three times (post 4, "What this costs") |
| KMP-6 | interop | Objective-C generics "can only be defined on classes, not on interfaces … or functions"; Swift export erases generics | DOC [K1], [K2] | MISSING | Undra is **stricter**: no generics at all (E25, E0002). M-6 |
| KMP-7 | interop | Same-name classes from different packages are renamed by an algorithm that "is not stable yet and can change between Kotlin releases" | DOC [K2] | SOLVED | E0050 / E0051 fail at bindgen; ids derive from names (SPEC §1.1, §12) |
| KMP-8 | memory | Swift objects that cross into Kotlin are reclaimed "only during the garbage collection". Retain cycles spanning Kotlin and Objective-C "cannot be reclaimed". Under allocation spikes "the GC forces a stop-the-world phase" | DOC [K3] (29 May 2026), [K4] (17 Apr 2025) | SOLVED | E6 (no tracing GC; the core holds host code only as port registrations, which are released with documented lifetimes, SPEC §6 host contract 1) |
| KMP-9 | packaging | Several KMP frameworks in one iOS app duplicate their dependencies, and "any state passed by different modules through the same dependency won't be connected". The fix is an umbrella framework, whose constraint is that the app "can't use only some of the feature modules" | DOC [K7] (12 Mar 2026) | MISSING | Undra is **worse today**: one core per process and unprefixed symbols (E7). M-12 |
| KMP-10 | tooling | Kotlin/Native build time is a standing cost. JetBrains' roadmap puts "improving build speeds" first among its iOS priorities, and its compile-time guide lists more than a dozen settings to tune | DOC [K5] (3 Sep 2026), [K8] | PLANNED | B1–B3 (edit the core without rebuilding the app on all three platforms). Rust is not fast either, and a 20k-line rebuild is unmeasured (`bench/RESULTS.md:598`); recommend an E1 row |
| KMP-11 | dev loop | Compose Hot Reload supports desktop JVM targets only; JetBrains suggests using "the desktop app as your sandbox" | DOC [K6] (1 Oct 2026) | PLANNED | E8 today (iOS and web live core); B1 (Android), B2 (reconnect), B3 (keep state) |
| KMP-12 | web | Kotlin/Wasm is "still in Beta" and needs WasmGC. Kotlin/JS exports collections as `KtList` / `KtMap` wrappers, and `Long` needs compiler flags | DOC [K9] (1 Sep 2026), [K10] (23 Sep 2026) | SOLVED | E9 |
| KMP-13 | packaging | CocoaPods is one of KMP's documented iOS integration methods [K14], and CocoaPods trunk stops accepting new podspecs on 2 Dec 2026 [X1] | DOC [K14] (21 Jul 2026); DOC [X1] | SOLVED | E10 (SwiftPM and XCFramework only). KMP also offers SwiftPM, so this is a migration chore for KMP teams on Pods, not a blocker |
| KMP-14 | state/data | The state and data layers are assembled from libraries: Flow/ViewModel, Ktor, SQLDelight or Room, and Store. Room has no web target in Google's table | DOC [K12]; RAW [K18], [K19]; DER | SOLVED | E11 (one cache, optimistic and offline semantics, tested on three platforms). The SQL part is something KMP does better: §9, G3 |
| KMP-15 | state | Shared `StateFlow` state needs a Swift-side collector before SwiftUI observes it (SKIE or KMP-NativeCoroutines expose `AsyncSequence`; Swift export's AsyncSequence mapping is Alpha) | DOC [K1], [K16]; ledger K15 | SOLVED | E12 |
| KMP-16 | background | Google's API for persistent background work (WorkManager) is Android-only. We found no shared background-execution abstraction in KMP's documentation | DOC [X2] (26 Feb 2026); DER | MISSING | Undra has none either. M-5 |
| KMP-17 | shared UI | Compose Multiplatform for web is Beta, and CMP draws "all of the components … on a canvas" (relevant to teams that share UI) | DOC [K11] | N/A | Undra does not share UI (E24) |

**Where KMP still beats Undra (as of October 2026), and what to do:**

| # | Advantage | Basis | Recommendation |
|---|---|---|---|
| KMP-B1 | **Android has no boundary at all.** Shared Kotlin is just Kotlin on Android; Undra crosses JNI | ledger K13 | Do not claim otherwise. Publish device-measured Android rows (E1) so the cost of the crossing is a number |
| KMP-B2 | **Ecosystem and backing.** JetBrains plus Google's stated support; KMP-ready Room, DataStore, Paging, ViewModel, lifecycle and SQLite; Ktor; SQLDelight, which "verifies your schema, statements, and migrations at compile-time"; Store | DOC [K12]; RAW [K19], [K18] | Do not chase library parity. Close the structural gaps: G3 `Db` port, A5 migrations, Android adapters (finding 1) |
| KMP-B3 | **IDE.** The KMP plugin (IntelliJ IDEA and Android Studio, on macOS, Windows and Linux) offers "cross-language navigation and debugging for Swift and Kotlin" | DOC [K13] | M-8 (a documented and scripted debugger path into Rust); rust-analyzer is the Rust-side IDE |
| KMP-B4 | **Language fit.** For an Android-first team the shared code is in its daily language (judgement, ledger T17) | DER | Rust is the adoption wall (blueprint §02). Answer it with H1 (cookbook) and D1 (diagnostics). Do not hide it in the post |
| KMP-B5 | **Shared UI when wanted.** CMP is Stable on iOS, with SwiftUI and UIKit interop | DOC [K11], ledger K28 | N/A by decision. Say so in the post |
| KMP-B6 | **Desktop and server.** CMP desktop is Stable on Windows, macOS and Linux; Kotlin on the server shares models | DOC [K11]; ledger K26–K27 | C5 (JVM and macOS); M-11 (Windows); M-13 (server reuse doc) |
| KMP-B7 | **Older iOS.** Kotlin/Native's default minimum is iOS 15.0; Undra needs iOS 17 | DOC [K15] (19 Aug 2026); E26 | M-9 |
| KMP-B8 | **Breadth of what can be shared.** Any Kotlin, including generic classes, lambdas and objects returned from methods, and shared code that calls platform APIs directly (expect/actual) | DOC [K2]; DER | M-3, M-4, M-6. Platform API access stays a port by design (R12), and that is a stated trade |
| KMP-B9 | **Apple secondary platforms.** watchOS and tvOS are Tier 2 Kotlin/Native targets | DOC [K15] | M-15 |

---

## 4. UniFFI

**Snapshot (as of October 2026).** Mozilla's "multi-language bindings generator for Rust". The latest
version on crates.io is 0.32.2; the crate was last updated on 23 Sep 2026 [U10]. The README says "used extensively by Mozilla in Firefox mobile and
desktop" and "ready for production use, but UniFFI is a long way from a 1.0 release" [U1]. First-party
languages are Kotlin, Swift, Python and Ruby; third parties cover C#, Go, Dart, Java, Node, Haskell, Kotlin
Multiplatform (Gobley), and JavaScript/WASM plus React Native (uniffi-bindgen-react-native) [U1]. The
license is MPL-2.0 [U9].

| Id | Group | Limitation a team hits | Basis · source | Undra | Evidence / piece |
|---|---|---|---|---|---|
| UNI-1 | state | As far as its documentation goes, UniFFI has no observable state, cache or dev loop. Keeping a SwiftUI screen in sync with Rust state is the app's design problem (polling, or callback interfaces that push) | RAW [U1], [U2]; DER (ledger U10, U11) | SOLVED | E12, E13 |
| UNI-2 | perf | Every call is an FFI call. "Strings, Optionals and Records … are lowered to a byte buffer called a RustBuffer" and lifted on the other side, so every read of Rust state is a call | RAW [U7]; DER | SOLVED | E5 (S11: 1,000 reads, zero crossings) |
| UNI-3 | async | "We don't directly support cancellation in UniFFI even when the underlying platforms do", and there is no way to raise a platform cancellation error. The Swift helper has `fatalError("Cancellation not supported yet")` for a cancelled call status | RAW [U4]; RAW [U5] `Helpers.swift` | SOLVED | E14 (S06) |
| UNI-4 | errors | Panics: in the Swift bindings a function that does not return `Result` is called with `try!`, and a Rust panic surfaces as `UniffiInternalError.rustPanic`, so a panic there stops the process. Kotlin throws an unchecked `InternalException` | RAW [U5] `macros.swift` (`is_try`: `try!`), `Helpers.swift`; RAW [U6]; DER (the consequence) | SOLVED | E4 (S17, through the generated bindings). Caveat: on wasm Undra traps too and recovers from a snapshot |
| UNI-5 | compat | The Swift bindings stop the process (`fatalError`, "UniFFI API checksum mismatch") when the library and bindings disagree | RAW (ledger U08, `wrapper.swift`) | SOLVED | E15 (typed `UndraSchemaMismatchError`, S16) |
| UNI-6 | packaging | "UniFFI doesn't provide an end-to-end packaging solution": it does not compile Rust for Android or package the bindings into an `.aar` | RAW [U2] | SOLVED | E16, E10. Build-system hooks are D3 |
| UNI-7 | interop | Generated Kotlin loads through JNA. A JNI-based generator is "experimental" in the unreleased CHANGELOG ("use at your own risk") | RAW [U3]; ledger U06 | SOLVED | E17 |
| UNI-8 | reach | Web and JavaScript come only from a third-party generator | RAW [U1] | SOLVED | E9 |
| UNI-9 | data | No cache, mutation, offline or persistence layer | DER (absent from [U1], [U2]) | SOLVED | E11 |
| UNI-10 | types | No stream type: the built-in type table has async functions but no streams | RAW [U13] | SOLVED | E2 (S07) |
| UNI-11 | runtime | "There's no requirement for a Rust event loop": the foreign side supplies the executor, and the library brings its own timers and blocking work | RAW [U4]; DER | SOLVED | E18 (an executor, Timer port and `spawn_blocking` that work on wasm too) |
| UNI-12 | testing | No fakes for platform I/O; tests are plain Rust and you build the doubles | DER | SOLVED | E19 |
| UNI-13 | dev loop | Each change means rebuilding the library and the app; no dev server is documented | DER | PLANNED | E8 today; B1–B3 |
| UNI-14 | tooling | No inspector or devtools documented | DER | PLANNED | B4 |
| UNI-15 | legal | MPL-2.0, a file-level copyleft that some legal reviews flag | RAW [U9]; DER | SOLVED | E20 |

**Where UniFFI still beats Undra:**

| # | Advantage | Basis | Recommendation |
|---|---|---|---|
| UNI-B1 | **Objects, callbacks and foreign traits.** "Objects can be freely passed as arguments and returned as values", and traits marked `with_foreign` can be implemented in Swift or Kotlin and passed into Rust | DOC [U8] | **M-3, M-4.** This is the first wall a UniFFI migrant hits (E0064, E0004) |
| UNI-B2 | **Languages.** Python and Ruby first-party; C#, Go, Dart, Java and Node third-party | RAW [U1] | Not proposed (M-17). Undra's value is the state model on three UIs; say so |
| UNI-B3 | **Production record.** Firefox on mobile and desktop | RAW [U1] | Nothing substitutes for time. Dogfood (R10) and publish adopters when they exist |
| UNI-B4 | **Smaller to adopt.** Bindings with no runtime and no state model; it fits apps that keep their own architecture | DER (ledger U14) | Make L1 adoption (pure functions, no stores) a first-class cookbook page (H1) |
| UNI-B5 | **Richer built-in types.** `HashSet`, `SystemTime`, `Duration`, borrowed `&[u8]`, and (unreleased) zero-copy `&mut [u8]` | RAW [U13], [U3] | C3 (type coverage) should cover `HashSet` and borrowed bytes |
| UNI-B6 | **Several crates in one app.** Symbols are crate-namespaced, so independent UniFFI libraries can coexist (DER from the generated names; not verified in a two-library app) | DER | M-12 |

---

## 5. Crux

**Snapshot (as of October 2026).** Red Badger's Elm-style architecture. `crux_core` 0.20.0 (crates.io, 7 Aug
2026) [C5]. The README says: "pre-1.0 and under active development. It is production-ready, but occasional
breaking changes to the API can be expected" [C1]. The FFI is generated by BoltFFI, the foreign types by
facet-generate, and messages are serialized with Bincode [C1]. Shells: SwiftUI, Compose, React/Vue, Leptos,
Yew, and C# through BoltFFI [C1].

| Id | Group | Limitation a team hits | Basis · source | Undra | Evidence / piece |
|---|---|---|---|---|---|
| CRUX-1 | perf/state | The shell calls `view()` and receives the **complete** view model, serialized across the boundary, after every render request. Photoroom found that "it would be very hard for the UI to understand what exactly changed", replaced the Render capability, and built key-path changes generated by diffing two view models | RAW [C1]; 3P [C4] (15 Jan 2026) | SOLVED | E5 (per-signal change-sets; recorded keyed ops; S09, S10). Caveat: `Computed<Vec<T>>` still crosses whole until E2 |
| CRUX-2 | effects | The core cannot do I/O, and "each effect variant requires corresponding shell implementation" on every platform | DOC [C2] | PLANNED | E22: Undra ships the adapters on iOS and web, **but not on Android**. C4 / finding 1 |
| CRUX-3 | effects | Middleware, Crux's way to handle effects in Rust, "can't run in the current wasm path (it uses `std::thread::spawn`)" | DOC [C3] | SOLVED | E18 |
| CRUX-4 | ergonomics | Commands "do not have model access"; every state change goes back through an event | DOC [C2] | SOLVED | E12 (async methods write signals in place). The trade-off is that Crux's `update` is easier to reason about exhaustively (post 2) |
| CRUX-5 | capabilities | The published capabilities are Render, Http, KeyValue and Time. SSE and PubSub are examples to copy | RAW [C1] | PLANNED | E22 (ten standard ports); G2 (WebSocket and SSE), G3 (SQL) |
| CRUX-6 | data | We found no cache in the published capabilities or in crux_http's documentation | DER (ledger U33) | SOLVED | E11 |
| CRUX-7 | shell code | Each platform's shell is a hand-written loop: send the event, perform and resolve effects, read the view model | RAW [C1] | SOLVED | E1, E12 (generated stores; no loop to write) |
| CRUX-8 | dev loop | No live core reload is documented | DER | PLANNED | E8; B1–B3 |
| CRUX-9 | tooling | No event-log inspector or time travel is documented, although events are data | DER | PLANNED | B4 |

**Where Crux still beats Undra:**

| # | Advantage | Basis | Recommendation |
|---|---|---|---|
| CRUX-B1 | **Tests and previews without fakes.** "Tests can act as another Shell … No need for fakes, mocks or stubs." The view model is a plain value, so a SwiftUI preview needs only data | RAW [C1] | F1 (preview cores built from fakes or recorded change-sets) closes the preview half. Undra's tests keep fakes by design (R12) |
| CRUX-B2 | **A pure `update`** that can be read top to bottom (judgement) | DER (ledger U34) | Do not imitate it; describe the trade in the post |
| CRUX-B3 | **Rust and C# shells.** Leptos and Yew can import the core directly; C# shells come through BoltFFI | RAW [C1] | Not planned. Note it as a v2 question ("a Rust UI over an Undra core") |
| CRUX-B4 | **Production stories.** Proton and Photoroom | RAW [C1] | As UNI-B3 |

---

## 6. React Native (New Architecture)

**Snapshot (as of October 2026).** React Native 0.87 is the latest version [R4]. The New Architecture has
been the default since 0.76 [R1] and cannot be disabled since 0.82, with removal of the legacy code
scheduled [R2]. JSI "allows JavaScript to hold a reference to a C++ object … without serialization costs" [R1].
The minimum iOS has been 15.1 since 0.76 [R16].

| Id | Group | Limitation a team hits | Basis · source | Undra | Evidence / piece |
|---|---|---|---|---|---|
| RN-1 | threading | "For most React Native applications, your business logic will run on the JavaScript thread … If the JavaScript thread is unresponsive for a frame, it will be considered a dropped frame" | DOC [R3] (12 Aug 2026) | SOLVED | E23, E13 (core off the main thread; merged, frame-aligned delivery; our numbers in `docs/HIGH_FREQUENCY.md`) |
| RN-2 | native logic | Logic that must be native is a Turbo Native Module: a typed spec, Codegen, and a hand-written implementation per platform (Kotlin or Java; Objective-C++ in the iOS guide) | DOC [R5] (4 Sep 2026) | PLANNED | For native-UI apps: one Rust implementation (E1). For RN UI teams: G1 |
| RN-3 | engine | Hermes has no `WebAssembly`. The request "WASM support within Hermes?" has been open since 4 Dec 2020 (#429), so a wasm-compiled shared core cannot run inside Hermes | DOC [R6] | PLANNED | G1 (ADR-038). It must be a TurboModule/JSI over the native C ABI, not the wasm transport, reusing the TS mirror and codecs |
| RN-4 | dev loop | Fast Refresh keeps component state, but "if you edit a file that's imported by modules outside of the React tree, Fast Refresh will fall back to doing a full reload", and shared logic usually lives in exactly those files | DOC [R8] (12 Aug 2026) | PLANNED | B3 (today `undra dev` loses state too, E8) |
| RN-5a | reach | Web is an out-of-tree platform (React Native for Web, by necolas) | DOC [R9] (12 Aug 2026) | SOLVED | E9 (first-party web runtime; UI is React itself) |
| RN-5b | reach | Windows and macOS are out of tree (Microsoft), as are tvOS and visionOS (community, Callstack) | DOC [R9] | PLANNED | C5 (JVM and macOS samples). Windows: M-11. tvOS and visionOS: M-15 |
| RN-6 | background | Headless JS, React Native's background-task mechanism, is documented for Android only | DOC [R11] (12 Aug 2026) | MISSING | M-5 |
| RN-7 | brownfield | Adding RN to an existing iOS app goes through CocoaPods ("We use it to add the actual React Native framework code"), whose trunk becomes read-only on 2 Dec 2026 | DOC [R10] (12 Aug 2026); DOC [X1] | SOLVED | E10, E16 (`undra adopt`; SwiftPM) |
| RN-8 | data | Server-state semantics come from JS libraries; TanStack Query pauses queries offline and resumes them on reconnect. An app with both native and RN screens implements the data layer twice | DOC [R18]; DER | SOLVED | E11 (one implementation; with G1, the same one under RN) |

**Where React Native still beats Undra:**

| # | Advantage | Basis | Recommendation |
|---|---|---|---|
| RN-B1 | **One React UI for iOS and Android** (screens are host views), with Meta and Expo behind it | ledger T03; DOC [R9] | G1 puts Undra *under* RN instead of competing with it |
| RN-B2 | **Over-the-air updates of logic.** EAS Update ships "non-native pieces (such as JS, styling, and images)" without a new binary. Undra's core is native, so its logic cannot be updated over the air on mobile | DOC [R14] | Not feasible for native code under store rules. Do not promise it; state it in the post (M-18) |
| RN-B3 | **DevTools.** Console, breakpoints, network, performance, memory, component inspector and profiler | DOC [R12] (12 Aug 2026) | B4. Make the network and port log the first panel, since that is what RN users will compare it with |
| RN-B4 | **Built-ins and ecosystem.** WebSocket is built in [R13]; `expo-sqlite` runs on Android, iOS and macOS, and web is alpha [R15] | DOC | G2, G3 |
| RN-B5 | **iOS 15.1** | DOC [R16] | M-9 |

---

## 7. Flutter

**Snapshot (as of October 2026).** Flutter 3.47 (documentation updated 22–29 Sep 2026). It deploys to iOS 15+, Android
24+, web, Windows, macOS and Linux [F7]. It has its own renderer (Impeller) and widgets [F1].

| Id | Group | Limitation a team hits | Basis · source | Undra | Evidence / piece |
|---|---|---|---|---|---|
| FL-1 | UI | Flutter paints its own widgets with its own engine "rather than deferring to those provided by the system" | DOC [F1] (ledger T01) | SOLVED | E24 (by design) |
| FL-2 | seam | Platform channels are asynchronous, must be invoked on the platform's main thread, and serialize with `StandardMessageCodec`. Pigeon is recommended for type safety | DOC [F2] (29 Sep 2026) | PLANNED | For native-UI apps: E1, E17, E23. For Flutter UI teams: G4 (after G1) |
| FL-3 | platform views | Under hybrid composition "Flutter merges the raster thread into the platform thread … potentially causing lower application FPS and frame drops" | DOC [F6] (29 Sep 2026) | SOLVED | E24 (by design) |
| FL-4 | dev loop | Hot reload does not pick up native code. Changing an enum into a class, changing generic type parameters, `main()`, `initState()` and global initializers all need a restart | DOC [F3] (29 Sep 2026) | PLANNED | E8; B1–B3 (the Rust core reloads without rebuilding the app) |
| FL-5 | add-to-app | "Packing multiple Flutter libraries into an application isn't supported"; mobile is multi-engine only | DOC [F4] (31 Jul 2026) | MISSING | Undra has the same one-core-per-process limit (E7). M-12 |
| FL-6 | web | "Flutter is not suitable for static websites with text-rich flow-based content", and its output "doesn't align with what search engines need to properly index" | DOC [F5] (21 Sep 2026) | SOLVED | E9 (DOM and React UI; SSR-safe hooks) |
| FL-7 | Rust | Rust under Flutter means a third-party bindings generator (flutter_rust_bridge, a "Flutter Favorite"), with no state mirror or data layer | RAW [F12]; DER | PLANNED | G4 |
| FL-8 | previews | The widget previewer (stable in 3.47) does not support `dart:ffi` or native plugins, so widgets bound to a Rust core cannot be previewed live | DOC [F9] (29 Sep 2026) | PLANNED | F1 (preview cores), and for Flutter after G4 |
| FL-9 | a11y | Accessibility goes through a semantics tree that Flutter maintains | DOC (ledger T14) | SOLVED | E24 (by design) |

**Where Flutter still beats Undra:**

| # | Advantage | Basis | Recommendation |
|---|---|---|---|
| FL-B1 | **One UI codebase on six platforms**, with Google behind it | DOC [F7] | N/A by decision. Flutter UI teams get G4 |
| FL-B2 | **DevTools:** inspector, performance, CPU, memory, network, debugger, logging, app size, deep links | DOC [F8] (31 Jul 2026) | B4 |
| FL-B3 | **Hot reload that keeps state**, and a stable widget previewer | DOC [F3], [F9] | B3, F1 |
| FL-B4 | **Errors in the UI framework do not end the app.** In release mode a failed widget "shows a gray background" | DOC [F10] (29 Sep 2026) | Undra matches this for calls (E4) but not for a panicking computed, which holds back its store's deliveries. A3 (per-signal isolation) is planned |
| FL-B5 | **Packages:** drift (SQLite, migrations, every platform, "Flutter Favorite"); `web_socket_channel` (tools.dart.dev) | DOC [F13], [F14] | G3, G2 |
| FL-B6 | **flutter_rust_bridge's surface:** "arbitrary types", closures, `&mut`, traits, and "Rust can also call Dart" | RAW [F12] | M-3, M-4, M-6. G4 must not ship with a narrower surface than the bridge it replaces |

---

## 8. Capacitor / Ionic (brief)

**Snapshot.** Capacitor v8: "a cross-platform native runtime" for "Web Native apps", with native features
reached through plugins [CA1].

| Id | Limitation | Basis | Undra | Evidence |
|---|---|---|---|---|
| CAP-1 | The UI is a web app in a native container | DOC [CA1] | SOLVED | E24 |
| CAP-2 | Plugin data must be JSON-serializable ("any data that can be JSON serialized"), and calls are asynchronous | DOC [CA2] | SOLVED | E1 (typed binary wire, synchronous and asynchronous calls) |
| CAP-3 | Each native feature is a plugin written per platform (Swift on iOS, Java on Android) | DOC [CA1] | PLANNED | E22; Android adapters, C4 |

**Where Capacitor beats Undra.** A web team ships mobile apps with what it already has, and web content
updates without store review (DER). Undra does not try to serve that team.

---

## 9. The status quo: "write the logic three times"

These rows are judgement (DER): they are the premise of the blueprint (§00, "the code below the UI is where
the three platforms disagree"). We know of no primary source that measures drift between independent
implementations, and the post must say so.

| Id | Limitation | Undra | Evidence / piece |
|---|---|---|---|
| SQ-1 | The cache, retry policy, offline queue and session refresh are written and tested three times, and they drift | SOLVED | E11; S12–S14 run the same core on three platforms |
| SQ-2 | Each platform has its own error model | SOLVED | E4, E3 |
| SQ-3 | A fix ships three times, on three schedules | SOLVED | One core (E1) |
| SQ-4 | Logic tests need three sets of time and network fakes | SOLVED | E19 |
| SQ-5 | Persistence formats and migrations differ per platform | PLANNED | A5 (one migration story), G3 |

**Where the status quo beats Undra:** no new language, toolchain or hiring; the best native tooling (Xcode,
Android Studio, browser devtools) with no boundary to debug across; full platform API access in each
language; and no framework risk. Recommendation: the post's "when not to choose Undra" section should
start here.

---

## 10. The matrix

Rows are capabilities a team needs. Cells: **yes**, **part.** (partial), **no**, **lib** (available from
third-party libraries, not part of the tool; not assessed in depth), **n/a** (does not arise, for example when
logic and UI share a language), and **—** (not assessed, or not measured by us, so no claim). The Undra v1.2
column is the plan as approved (Amendment A), not a promise; each cell names its piece. Footnotes follow the
table.

| # | Capability | KMP | UniFFI | Crux | RN | Flutter | Undra today | Undra by v1.2 (planned) |
|---|---|---|---|---|---|---|---|---|
| 1 | Typed domain errors across the boundary, in each language's idiom | part. | yes | part. | n/a | n/a | yes | yes (+A4) |
| 2 | A bug in shared logic does not end the app | part. | part. | — | — | yes | yes native · part. web | yes (+A3) |
| 3 | UI reads shared state without crossing a language boundary | part. | no | yes | yes | yes | yes | yes |
| 4 | Updates cost O(change), including large lists | part. | no | no | n/a | n/a | part. | yes (E2) |
| 5 | Shared state observable natively (StateFlow, `@Observable`, hooks) | part. | no | part. | yes | yes | yes | yes |
| 6 | Cancellation from the UI, and streams with backpressure | part. | no | part. | n/a | n/a | yes | yes |
| 7 | Server-state cache with optimistic updates and rollback | lib | no | no | lib | — | yes | yes |
| 8 | Offline mutation queue that survives a restart intact | lib | no | no | lib | — | part. | part. (A5; M-7) |
| 9 | Persisted state with migrations across schema changes | yes | no | no | lib | lib | no | yes (A5) |
| 10 | Paged and infinite lists, and polling | yes | no | no | lib | lib | no | part. (E3 needs an ADR; M-1) |
| 11 | Structured local database (SQL) | yes | lib | no | yes | yes | no | yes (G3) |
| 12 | WebSocket and real-time streams | yes | lib | part. | yes | yes | no | yes (G2) |
| 13 | OS background execution (BGTaskScheduler, WorkManager) | part. | no | no | part. | — | no | no (M-5) |
| 14 | Default HTTP, storage and connectivity adapters on iOS, Android and web | yes | no | part. | yes | yes | part. | yes (C4) |
| 15 | Live reload of shared logic on a device, keeping state | part. | no | — | part. | yes | part. | yes (B1–B3) |
| 16 | State inspector, transaction timeline, time travel | no | no | no | part. | part. | no | yes (B4) |
| 17 | Step-debug from UI code into shared code | yes | — | — | yes | yes | no | no (M-8) |
| 18 | Previews and UI tests without the real core | part. | part. | yes | yes | part. | no | yes (F1) |
| 19 | Deterministic tests (virtual time, fake I/O) | yes | part. | yes | — | — | yes | yes (+F2) |
| 20 | Compatibility check between bindings and library | n/a | yes | yes | part. | n/a | yes | yes |
| 21 | Generated Swift passes native review | part. | yes | part. | n/a | n/a | yes | yes |
| 22 | Generics across the boundary | part. | no | — | n/a | n/a | no | no (M-6) |
| 23 | Objects, callbacks and listeners as arguments and returns | yes | yes | n/a | n/a | n/a | part. | part. (M-3, M-4) |
| 24 | Supports iOS 15 and 16 | yes | — | — | yes | yes | no | no (M-9) |
| 25 | First-party web target with a DOM UI | part. | part. | yes | part. | part. | yes | yes |
| 26 | Desktop | yes | yes | part. | part. | yes | part. | part. (C5) |
| 27 | Reuse of the shared code on a server | yes | part. | part. | yes | part. | part. | part. (M-13) |
| 28 | React Native UI on top | no | part. | no | yes | no | no | yes (G1) |
| 29 | Flutter UI on top | no | part. | no | no | yes | no | part. (G4, after G1) |
| 30 | Two independent libraries built with it in one app | part. | — | — | n/a | no | no | no (M-12) |
| 31 | Incremental adoption in an existing app | yes | yes | part. | part. | part. | part. | yes (D3, H1) |
| 32 | Build-system integration (Gradle, Xcode, web bundler) | yes | no | — | yes | yes | no | yes (D3) |
| 33 | Crash symbolication of shared code | — | — | — | — | — | no | no (M-2) |
| 34 | Size, startup and list performance measured on devices | — | — | — | — | — | part. | yes (E1) |
| 35 | Ecosystem, backing, production record | yes | yes | part. | yes | yes | no | no |
| 36 | Shared UI if the product wants it | yes | no | no | yes | yes | n/a | n/a |

**Footnotes** (K = KMP, U = UniFFI, C = Crux, R = React Native, F = Flutter, Ut = Undra today, U2 = Undra by
v1.2; source ids are in §12, Undra evidence ids in §2).

1. K: only `@Throws` exceptions become Swift errors; the rest terminate [K2]. U: "a proper exception will be thrown if `Result::is_err()`" [U12]. C: errors are modelled as events and view-model data, and the bridge has no per-call error channel (DER from [C1]). R, F: logic and UI share a language; the native seam carries codes and messages ([R5]; [F2] `result.error("UNAVAILABLE", …)`). Ut: E4. U2: A4 (typed stream errors).
2. K: [K2] termination. U: `try!` with a panic in non-throwing Swift functions [U5]. C, R: not assessed. F: a failed widget is replaced and the app continues [F10]. Ut: native panics are contained; wasm traps and recovers from a snapshot (E4, S17). U2: A3 (a panicking computed is isolated per signal).
3. K: no boundary on Android; on iOS reads call into the Kotlin/Native runtime and collections are copied [K2]. U: every read is a call [U7]. C: the shell holds the view model [C1]. Ut: E5 (S11).
4. K: copies on iOS [K2]. U: no change model (DER). C: whole view model [C1], [C4]. Ut: keyed `Signal<Vec<T>>` patches; computed lists cross whole (E5). U2: E2, derived keyed lists (ADR-039).
5. K: `StateFlow` on Android; Swift needs SKIE, KMP-NativeCoroutines, or Swift export (Alpha) [K1], [K16]. U: none (ledger U10). C: the view model is rendered by hand-written shell code [C1]. Ut: E12.
6. K: SKIE adds "Cancellation support" to Flow and suspend from Swift [K16]; the Objective-C path is "highly experimental" [K2]. U: no cancellation [U4], no stream type [U13]. C: effects support "streaming semantics" [C1]; cancellation is not assessed. Ut: E14, E2.
7. K: Store and the wider ecosystem [K18]. R: TanStack Query [R18]. F: not assessed. Ut: E11 (S12, S13).
8. K, R: lib (as 7). Ut: the queue persists and replays (S14), but after a restart the optimistic writes and `invalidates` targets are gone, and only idempotent mutations queue (E11, `undra-query/src/lib.rs:59, 65-67`). U2: A5 covers schema changes; restoring optimistic state after a restart is M-7.
9. K: SQLDelight "verifies your schema, statements, and migrations at compile-time" [K19]; Room and DataStore [K12]. R: `expo-sqlite` [R15]. F: drift's "schema migrations" [F13]. Ut: entries from another schema are dropped (E11, E31). U2: A5 (ADR-037).
10. K: Jetpack Paging is KMP-ready [K12]. R, F: lib (not assessed). Ut: `Lazy<T>` is rejected (E25), and interval refetch is not implemented (E11). U2: E3 assumes `Lazy<T>`, which needs an ADR first (M-1).
11. K: Room, SQLite, SQLDelight [K12], [K19]. U: any Rust SQLite crate, native only (DER). C: KeyValue only [C1]. R: `expo-sqlite`, web alpha [R15]. F: drift [F13]. U2: G3.
12. K: Ktor WebSockets on the Darwin, OkHttp, CIO and Js engines; "the Android engine does not support WebSockets" [K20]. C: SSE and PubSub example capabilities [C1]. R: built in [R13]. F: `web_socket_channel` [F14]. Ut: no WebSocket port (`undra dev` uses WebSocket only as a transport). U2: G2 (ADR-040).
13. K: WorkManager is Android-only [X2]; iOS work is platform code (DER). R: Headless JS, Android-only [R11]. F: not assessed. Ut, U2: nothing (M-5).
14. K: Ktor, DataStore [K12]. C: the shell implements each effect [C2]. R, F: built-ins and packages ([R13], [R15], [F14]). Ut: Android has only Clock, Rng, Log and Timer (E22). U2: C4 (finding 1: name it as a piece).
15. K: Compose Hot Reload, desktop JVM only [K6]. R: Fast Refresh falls back to a full reload for non-component modules [R8]. F: hot reload [F3]. Ut: iOS and web live core, Android none, state lost (E8). U2: B1, B2, B3.
16. R: DevTools has no app-state time travel [R12]. F: DevTools [F8]. Ut: a log stream (E29). U2: B4.
17. K: "cross-language navigation and debugging for Swift and Kotlin" [K13]. R, F: same-language debuggers ([R12], [F8]). Ut, U2: no documented path (M-8).
18. K: the KMP plugin's Compose previews [K13]; SwiftUI needs the framework built (DER). U: records are plain values, objects need the library (DER). C: the view model is a value; "No need for fakes, mocks or stubs" [C1]. F: no `dart:ffi` in previews [F9]. Ut: `UndraCore.shared` is a placeholder whose constructors throw (`docs/SWIFT_ERRORS.md` "Before a core is loaded"). U2: F1.
19. K: `kotlinx-coroutines-test` virtual time, multiplatform [K21]. U: plain Rust tests (DER). C: tests act as the shell [C1]. Ut: E19. U2: F2 (record and replay).
20. K: one compiled artifact (n/a). U: checksums, with a Swift `fatalError` on mismatch (ledger U08). C: generated types fail to compile (ledger U36). R: Codegen from typed specs at build time [R5]. Ut: E15.
21. K: Objective-C header; Swift export Alpha; SKIE [K1], [K2], [K17]. U: Swift classes and functions mirroring the Rust interface (ledger U13). C: types only; the shell loop is hand-written [C1]. Ut: E1 (R3, golden files).
22. K: classes only, or erased [K1], [K2]. U: no generics in the documented type list [U13] (DER: absence). Ut: E0002 (E25).
23. K: classes, objects and function types are exported [K2]. U: objects as arguments and returns; foreign traits [U8]. F, via flutter_rust_bridge: "arbitrary types", Rust calls Dart [F12]. Ut: objects only through constructors (E0064); host code only through global ports (E0004).
24. K: iOS 15.0 default [K15]. U, C: no floor of their own (not assessed). R: 15.1 [R16]. F: iOS 15 [F7]. Ut: iOS 17 (E26).
25. K: Kotlin/Wasm Beta, Kotlin/JS wrappers [K9], [K10]. U: third party [U1]. C: wasm core with React, Vue and Rust shells [C1]. R: out of tree [R9]. F: canvas; not for text-rich or SEO content [F5]. Ut: E9.
26. K: CMP desktop Stable [K11]. U: Firefox desktop; Python [U1]. C: Rust shells; macOS Swift (DER from [C1]). R: out of tree, Microsoft [R9]. F: Windows, macOS, Linux [F7]. Ut: E30. U2: C5 (JVM and macOS); Windows is M-11.
27. K: Kotlin on the server (ledger K27). U, C: the same Rust crate (DER). R: JS/TS on Node (DER). F: Dart on the server (DER). Ut: TS runtime on Node 20+ (E9); Rust records reusable in a Rust server (DER, undocumented). U2: M-13.
28. U: uniffi-bindgen-react-native, third party [U1]. Ut: none. U2: G1 (ADR-038).
29. U: third-party Dart bindings [U1]. U2: G4 after G1 (Amendment A).
30. K: not recommended; umbrella framework [K7]. F: "isn't supported" [F4]. Ut: E7. U2: M-12.
31. K: integration methods [K14]. U: bindings only. C: one core and a shell loop per app (DER). R: brownfield via CocoaPods [R10]. F: add-to-app limits [F4]. Ut: `undra adopt` writes a core and instructions, and the edits are manual (E27). U2: D3, H1.
32. K: Gradle, plus an Xcode build-phase script that the IDE plugin applies by default [K14]. U: "doesn't provide an end-to-end packaging solution" [U2]. R: Gradle plugin and CocoaPods [R10]. Ut: E27. U2: D3.
33. Competitors: not assessed. Ut: E28. U2: M-2.
34. Competitors: not measured by us, so no claim. Ut: host-measured core-side rows (E21). U2: E1 device rows.
35. K: [K12], [K22]. U: [U1]. C: pre-1.0, with production write-ups [C1]. R, F: Meta and Expo, Google (DER). Ut: v1.0 from one team (ledger, posts 1–3).
36. K: CMP [K11]. R, F: (ledger T01, T03). Ut: N/A by decision (E24).

---

## 11. What this means for the plan

### 11.1 The MISSING items, ranked by how often a team would hit them

The ranking is judgement (DER): how many teams evaluating Undra in the first month would run into the item.
Sizes: **S** (a piece under a week for one implementer), **M** (one worktree with an ADR or a cross-platform
surface), **L** (several pieces, or a wire/ABI change).

| Rank | Id | What is missing | What "solved" looks like | Size | Alternatives that have it |
|---|---|---|---|---|---|
| 1 | M-1 | **Paged and infinite lists, plus polling.** `Lazy<T>` is rejected in v1 (E25), yet E3 is scoped as "ergonomics". SPEC §9's `interval_ms` is not implemented (E11) | An ADR for `Lazy<T>`: the wire already reserves op 2 "lazy list invalidated" and `TypeRef::Lazy`. Add `#[undra::query(infinite, cursor = ..)]` producing a keyed lazy list with `loadMore()` / `hasMore` signals; generated paging helpers (Paging-style on Kotlin, `onAppear`-driven on SwiftUI, a hook on React); `interval = "30s"` on queries driven by the Timer port; contract scenarios for page, refetch and invalidate | L (polling alone: S) | KMP (Jetpack Paging), RN and Flutter libraries |
| 2 | M-2 | **Crash symbolication.** No symbol files (E28) | `undra build --release` writes `symbols/`: unstripped Android `.so` files zipped for Play Console and Crashlytics, DWARF kept for the iOS static library so the app's dSYM covers Rust frames, and a wasm DWARF or name-section sidecar. A doc page per crash reporter, and a test that a symbolicated panic backtrace names the Rust file and line | S | Native toolchains; competitors not assessed |
| 3 | M-3 | **Objects as parameters and returns** (E0064) | An ADR adding `TypeRef::Object(name)` that crosses as a handle, with an ownership rule (a returned handle is owned by the caller and released by close or finalizer). Generated code wraps the handle in the generated class. Contract scenario: return, pass, release, stale use | M | KMP, UniFFI, flutter_rust_bridge |
| 4 | M-4 | **Host callbacks and listener objects as arguments** (E0004; ports are global per id) | An ADR for foreign trait objects: `#[undra::callback] trait Listener` passed as `Arc<dyn Listener>`, implemented in Swift, Kotlin or TS. It reuses the port call path with an instance handle, carries the host contract's threading rules, and is released when the Rust side drops it | M–L | UniFFI (foreign traits), KMP (lambdas), flutter_rust_bridge |
| 5 | M-5 | **OS background execution** (BGTaskScheduler, WorkManager, a service-worker sync) | `UndraCore.runInBackground(deadline:)` on each runtime loads the core if needed, replays the offline queue, refetches `persist` queries, snapshots, and returns before the deadline. It ships with an `UndraWorker` (WorkManager `CoroutineWorker`), a `BGTaskScheduler` registration helper, and a contract scenario with a fake deadline | M | Partly: KMP (platform code), RN (Android-only Headless JS) |
| 6 | M-6 | **Generic instantiations** (E0002) | `#[undra::api] pub type TodoPage = Page<Todo>;` becomes a named record in the schema, so `Page<T>` is written once in Rust and each instantiation is a concrete, reviewable type on each platform. Generic *functions* stay rejected with a teaching error | M | KMP (partly), flutter_rust_bridge |
| 7 | M-7 | **Offline mutations that survive a restart intact.** Optimistic writes and `invalidates` are lost on restart (E11) | Persist each queue item's optimistic patch and invalidation targets with the item (versioned with ADR-037). Rehydrate them before the first frame, and roll them back if the replay is rejected. Extend S14 with a restart step | M | RN and TanStack with persisters (lib) |
| 8 | M-8 | **Step-debugging into Rust** | A doc page and `undra doctor` checks for each platform: Xcode lldb stepping into a debug XCFramework, Android Studio native debugging of `libundra_core.so`, and Chrome DevTools DWARF for wasm. `undra build --debug-symbols` makes the right artifact | S–M | KMP plugin, and same-language debuggers in RN and Flutter |
| 9 | M-9 | **iOS 15 and 16** (E26) | An ADR for a generated-shape variant: `swift_observation = "observable-object"` emits `ObservableObject` + `@Published` stores (still Swift 6 strict concurrency), with the runtime's Observation-only paths behind availability checks, a golden case, and the Swift contract suite run on an iOS 16 simulator | M | KMP, RN, Flutter (all iOS 15) |
| 10 | M-10 | **Push and other OS events as standard ports** | A standard `Push` event port (token, payload, opened) with default adapters (APNs token from the app delegate, FCM on Android, Push API on web) and fakes | S | Native SDKs, and RN and Flutter packages |
| 11 | M-11 | **Windows development machines** (E30) | The CLI and its Android, web and JVM builds on Windows; `doctor` checks; a CI job | M | KMP plugin, Flutter, RN |
| 12 | M-12 | **Two Undra libraries in one app** (E7) | An ADR (ABI): each core exports one namespaced entry (`undra_<crate>_vtable()`) instead of global `undra_*` symbols. Runtimes hold one `UndraCore` per vtable, and the JNI class is per core. Decide this before anything is published (finding 5) | L | UniFFI (DER); KMP documents the same pain (umbrella) |
| 13 | M-13 | **Server reuse story** | A cookbook page and sample: share the core's records with an axum server through the same crate, and run the wasm core under Node for SSR with data (today `useUndra` is `undefined` on the server, E9) | S | KMP, RN |
| 14 | M-14 | **Tracing spans** (blueprint §12's Telemetry port, not in v1) | A `Telemetry` port that emits a span per command, transaction, query fetch and port call, with an OpenTelemetry-compatible adapter on each platform | M | Native SDKs |
| 15 | M-15 | **Apple secondary platforms** (a macOS slice, tvOS, visionOS, watchOS) | XCFramework slices plus `Package.swift` platforms. C5 should include the macOS slice, which is missing today (E30) | S–M | KMP (Tier 2 watchOS and tvOS), RN (out of tree) |
| 16 | M-16 | **Default parameter values** | `#[undra(default = ..)]` on method parameters, emitted as Swift and Kotlin default arguments and TS optional parameters | S | KMP with SKIE, UniFFI (`defaults.md`) |
| 17 | M-17 | **Python, Ruby, C#, Go hosts** | Not proposed. UniFFI serves these; the post should say Undra targets three UIs | — | UniFFI, Crux (C#) |
| 18 | M-18 | **Over-the-air logic updates on mobile** | Not proposed (native code under store rules). Document that the web core updates with the site | — | RN (EAS Update, JS only) |

### 11.2 Corrections to the existing plan

* **C4 → a named Phase-1 piece "android-adapters"** (finding 1). The adapters, the
  `UndraAndroid.load(context)` installer and the R8 rules are specified in
  `runtimes/kotlin/undra-runtime/android-adapters/README.md:21-44`. CI cannot see this gap: the contract
  suite supplies its own `Kv` and `Http` on every platform, and runs the Kotlin column on the JVM
  (`contract-tests/scenarios.md:27-36`). Add an Android instrumented smoke test that uses the default
  adapters.
* **E3 → re-scope as M-1 with an ADR** (`Lazy<T>` first). Add polling to it.
* **A5 → include M-7** (optimistic state and invalidations across a restart), not only across a schema
  change.
* **C3 → extend with newtypes (E0007), `HashSet` and borrowed bytes.** Objects, callbacks and generics
  (M-3, M-4, M-6) need ADRs before C3's implementers touch the macros.
* **D3 → add M-2 (symbols) and M-8 (debugging).** They share the build plumbing.
* **E1 → add the blueprint's missing rows:** incremental core rebuild for a 20k-line core, idle memory,
  and wasm instantiate time.
* **G1 (ADR-038) → design constraint:** Hermes has no `WebAssembly` [R6], so the React Native runtime has to
  be JSI over the C ABI. M-12 (namespacing) changes that ABI too, so decide both together.
* **G4 → parity floor:** the Dart surface must not be narrower than flutter_rust_bridge's for the types
  Undra supports, or Flutter teams will not move (FL-B6).
* **H4 (the post) → do not reuse these blueprint claims.** The code does not back them as of today: "Time-travel any transaction in a devtools inspector" (B4 is planned); "The core runs in a Web
  Worker by default" (the generated web app uses `wasm-main`, `examples/playground/web/src/undra.ts:42`);
  "Lazy collections … a 50,000-row table is never serialized" (E25); "Newtypes stay typed" (E0007);
  "the optimistic state survives app restarts" (E11); "Kv is Rust-owned SQLite on native" (SPEC §0,
  ADR-014); a Telemetry or Push port (SPEC §8 lists ten ports, neither of these); "undra adopt adds the
  package and a bootstrap" (it writes instructions only, E27); command priorities (none in
  `undra-runtime`).

### 11.3 Where we should not try to "fix" the gap

Shared UI (N/A by decision); other host languages (M-17); OTA updates of native logic (M-18); matching
KMP's or RN's library ecosystems one library at a time (KMP-B2, RN-B4). Close the structural gaps (G2, G3,
the adapters) and let the core use Rust crates.

---

## 12. Sources

All were fetched on **2026-10-01** unless marked "ledger", which means fetched on 2026-09-30 and recorded in
`.10x/reviews/2026-09-30-blog-claims.md` with its fact-check. The "page date" is the date the page itself shows.

| Id | URL | Title | Page date |
|---|---|---|---|
| K1 | https://kotlinlang.org/docs/native-swift-export.html | Interoperability with Swift using Swift export | 28 Aug 2026 |
| K2 | https://kotlinlang.org/docs/native-objc-interop.html | Interoperability with Swift/Objective-C | 12 Aug 2026 |
| K3 | https://kotlinlang.org/docs/native-memory-manager.html | Kotlin/Native memory management | 29 May 2026 |
| K4 | https://kotlinlang.org/docs/native-arc-integration.html | Integration with Swift/Objective-C ARC | 17 Apr 2025 |
| K5 | https://kotlinlang.org/docs/native-improving-compilation-time.html | Tips for improving compilation time | 3 Sep 2026 |
| K6 | https://kotlinlang.org/docs/multiplatform/compose-hot-reload.html | Compose Hot Reload | 1 Oct 2026 |
| K7 | https://kotlinlang.org/docs/multiplatform/multiplatform-project-configuration.html | Choosing a project configuration (several shared modules) | 12 Mar 2026 |
| K8 | https://blog.jetbrains.com/kotlin/2025/08/kmp-roadmap-aug-2025/ | Kotlin Multiplatform roadmap, August 2025 update | Aug 2025 |
| K9 | https://kotlinlang.org/docs/wasm-overview.html | Kotlin/Wasm | 1 Sep 2026 |
| K10 | https://kotlinlang.org/docs/js-to-kotlin-interop.html | Use Kotlin code from JavaScript (`@JsExport`) | 23 Sep 2026 |
| K11 | https://kotlinlang.org/docs/multiplatform/faq.html | Kotlin Multiplatform FAQ (CMP stability, canvas) | 1 Oct 2026 |
| K12 | https://developer.android.com/kotlin/multiplatform | Kotlin Multiplatform (Android Developers) | 23 Sep 2026 |
| K13 | https://kotlinlang.org/docs/multiplatform/multiplatform-plugin-releases.html | Kotlin Multiplatform IDE plugin releases | 13 Jan 2026 |
| K14 | https://kotlinlang.org/docs/multiplatform/multiplatform-ios-integration-overview.html | iOS integration methods | 21 Jul 2026 |
| K15 | https://kotlinlang.org/docs/native-target-support.html | Kotlin/Native target support | 19 Aug 2026 |
| K16 | https://skie.touchlab.co/ | SKIE | n/a |
| K17 | https://touchlab.co/the-future-of-kmps-ios-interop | The future of KMP's iOS interop (Touchlab, 3P) | 12 Jun 2026 |
| K18 | https://raw.githubusercontent.com/MobileNativeFoundation/Store/main/README.md | Store5 README | n/a |
| K19 | https://raw.githubusercontent.com/sqldelight/sqldelight/master/README.md | SQLDelight README | n/a |
| K20 | https://ktor.io/docs/client-engines.html | Ktor client engines, Limitations | n/a |
| K21 | https://kotlinlang.org/api/kotlinx.coroutines/kotlinx-coroutines-test/ | kotlinx-coroutines-test | n/a |
| K22 | https://kotlinlang.org/docs/whatsnew1920.html | What's new in Kotlin 1.9.20 (ledger) | n/a |
| U1 | https://raw.githubusercontent.com/mozilla/uniffi-rs/main/README.md | UniFFI README | n/a |
| U2 | https://raw.githubusercontent.com/mozilla/uniffi-rs/main/docs/manual/src/Motivation.md | UniFFI Motivation | n/a |
| U3 | https://raw.githubusercontent.com/mozilla/uniffi-rs/main/CHANGELOG.md | UniFFI CHANGELOG (v0.32.1 2026-09-08; v0.32.0 2026-06-30; unreleased) | n/a |
| U4 | https://raw.githubusercontent.com/mozilla/uniffi-rs/main/docs/manual/src/futures.md | UniFFI async/futures ("Cancelling async code") | n/a |
| U5 | https://raw.githubusercontent.com/mozilla/uniffi-rs/main/uniffi_bindgen/src/bindings/swift/templates/Helpers.swift and …/macros.swift | UniFFI Swift templates (panic, `try!`, cancellation) | main, read 2026-10-01 |
| U6 | https://raw.githubusercontent.com/mozilla/uniffi-rs/main/uniffi_bindgen/src/bindings/kotlin/templates/Helpers.kt | UniFFI Kotlin template (`InternalException`) | main, read 2026-10-01 |
| U7 | https://raw.githubusercontent.com/mozilla/uniffi-rs/main/docs/manual/src/internals/lifting_and_lowering.md | Lifting and lowering (ledger) | n/a |
| U8 | https://mozilla.github.io/uniffi-rs/latest/types/interfaces.html | Interfaces/Objects | n/a |
| U9 | https://raw.githubusercontent.com/mozilla/uniffi-rs/main/LICENSE | Mozilla Public License 2.0 | n/a |
| U10 | https://crates.io/api/v1/crates/uniffi | crates.io: uniffi 0.32.2 | updated 2026-09-23 |
| U11 | https://raw.githubusercontent.com/mozilla/uniffi-rs/main/uniffi_bindgen/src/bindings/kotlin/templates/NamespaceLibraryTemplate.kt | JNA loading (ledger) | n/a |
| U12 | https://mozilla.github.io/uniffi-rs/latest/types/errors.html | Errors | n/a |
| U13 | https://raw.githubusercontent.com/mozilla/uniffi-rs/main/docs/manual/src/types/builtin_types.md | Built-in types | n/a |
| C1 | https://raw.githubusercontent.com/redbadger/crux/master/README.md | Crux README | n/a |
| C2 | https://redbadger.github.io/crux/part-2/effects.html | Crux book: Managed effects | n/a |
| C3 | https://github.com/redbadger/crux/tree/master/examples/counter-middleware | Crux example: Counter (Middleware) | n/a |
| C4 | https://www.photoroom.com/inside-photoroom/building-live-collaboration-in-rust-for-millions-of-users-part-3 | Photoroom, part 3 (3P) | 15 Jan 2026 |
| C5 | https://crates.io/api/v1/crates/crux_core | crates.io: crux_core 0.20.0 | updated 2026-08-07 |
| R1 | https://reactnative.dev/docs/the-new-architecture/landing-page | About the New Architecture | 22 Mar 2026 |
| R2 | https://reactnative.dev/blog/2025/10/08/react-native-0.82 | React Native 0.82 | 8 Oct 2025 |
| R3 | https://reactnative.dev/docs/performance | Performance overview (0.87) | 12 Aug 2026 |
| R4 | https://reactnative.dev/versions | React Native versions | n/a |
| R5 | https://reactnative.dev/docs/turbo-native-modules-introduction | Turbo Native Modules | 4 Sep 2026 |
| R6 | https://github.com/facebook/hermes/issues/429 | Hermes issue #429 "WASM support within Hermes?" (open) | opened 4 Dec 2020 |
| R8 | https://reactnative.dev/docs/fast-refresh | Fast Refresh | 12 Aug 2026 |
| R9 | https://reactnative.dev/docs/out-of-tree-platforms | Out-of-Tree Platforms | 12 Aug 2026 |
| R10 | https://reactnative.dev/docs/integration-with-existing-apps | Integration with Existing Apps | 12 Aug 2026 |
| R11 | https://reactnative.dev/docs/headless-js-android | Headless JS | 12 Aug 2026 |
| R12 | https://reactnative.dev/docs/react-native-devtools | React Native DevTools | 12 Aug 2026 |
| R13 | https://reactnative.dev/docs/network | Networking (WebSocket support) | 12 Aug 2026 |
| R14 | https://docs.expo.dev/eas-update/introduction/ | EAS Update | n/a |
| R15 | https://docs.expo.dev/versions/latest/sdk/sqlite/ | Expo SQLite | n/a |
| R16 | https://github.com/react-native-community/discussions-and-proposals/discussions/812 | "[0.76] iOS minimum OS version bump to 15.1" | 7 Aug 2024 |
| R18 | https://tanstack.com/query/latest/docs/framework/react/guides/network-mode | TanStack Query: Network Mode | n/a |
| F1 | https://docs.flutter.dev/resources/architectural-overview | Flutter architectural overview (ledger) | n/a |
| F2 | https://docs.flutter.dev/platform-integration/platform-channels | Writing custom platform-specific code | 29 Sep 2026 |
| F3 | https://docs.flutter.dev/tools/hot-reload | Hot reload | 29 Sep 2026 |
| F4 | https://docs.flutter.dev/add-to-app | Add Flutter to an existing app | 31 Jul 2026 |
| F5 | https://docs.flutter.dev/platform-integration/web/faq | Web FAQ | 21 Sep 2026 |
| F6 | https://docs.flutter.dev/platform-integration/android/platform-views | Hosting native Android views | 29 Sep 2026 |
| F7 | https://docs.flutter.dev/reference/supported-platforms | Supported deployment platforms (3.47) | 22 Sep 2026 |
| F8 | https://docs.flutter.dev/tools/devtools | Flutter and Dart DevTools | 31 Jul 2026 |
| F9 | https://docs.flutter.dev/tools/widget-previewer | Flutter Widget Previewer | 29 Sep 2026 |
| F10 | https://docs.flutter.dev/testing/errors | Handling errors in Flutter | 29 Sep 2026 |
| F11 | https://dart.dev/interop/c-interop | C interop using dart:ffi | 15 Sep 2025 |
| F12 | https://raw.githubusercontent.com/fzyzcjy/flutter_rust_bridge/master/README.md | flutter_rust_bridge v2 README | n/a |
| F13 | https://pub.dev/packages/drift | drift | n/a |
| F14 | https://pub.dev/packages/web_socket_channel | web_socket_channel | n/a |
| CA1 | https://capacitorjs.com/docs | Capacitor documentation (v8) | n/a |
| CA2 | https://capacitorjs.com/docs/plugins/ios | Capacitor iOS plugin guide | n/a |
| X1 | https://blog.cocoapods.org/CocoaPods-Specs-Repo/ | CocoaPods Specs repo: trunk read-only plan | 30 Nov 2024 |
| X2 | https://developer.android.com/develop/background-work/background-tasks/persistent | Persistent work (WorkManager) | 26 Feb 2026 |
| X3 | https://developer.apple.com/documentation/backgroundtasks | Background Tasks (read through the documentation JSON) | n/a |
| X4 | https://developer.apple.com/documentation/observation | Observation (introduced iOS 17.0, macOS 14.0) | n/a |

Ids R7 and R17 are not used. X3 backs the "BGTaskScheduler" sketch in M-5 ("keep your
app content up to date and run tasks requiring minutes to complete even if your app is in the background").
F11 is cited only for context: dart:ffi is how any Rust core reaches Dart, and G4 will sit on it.
