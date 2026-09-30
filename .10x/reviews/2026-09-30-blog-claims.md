# Blog claims ledger: every factual claim about another tool

Date: 2026-09-30. Scope: the four launch posts under `site/blog/` (`keel-vs-kotlin-multiplatform`,
`keel-vs-uniffi-and-crux`, `why-keel-keeps-your-ui-native`, `reads-never-cross-the-boundary`).
Purpose: let a fact-checker verify every statement a post makes about KMP, UniFFI, Crux, Flutter,
React Native, Compose Multiplatform (CMP) and SKIE, quickly and independently.

## How the basis column works

| Code | Meaning | How much to trust it |
|---|---|---|
| RAW | A raw file (README, docs source, template) fetched with `curl` on 2026-09-30. Text is verbatim. | High. Re-open the URL. |
| DOC | An official documentation page fetched on 2026-09-30 with the WebFetch tool. The tool returns a model-written summary of the page, not the page. | Medium. The quoted sentences were extracted by a small model; open the URL and confirm. |
| 3P | A third-party article (Touchlab, Photoroom) fetched the same way. | Medium, same caveat. |
| SRCH | Seen only in a web-search result summary, never on the primary page. | Low. Verify before relying on it. |
| BG | Background knowledge of the author, not re-checked on the day. | Low. Verify, or cut the sentence. |
| DER | A sentence derived from other rows (an architectural inference or a judgement). | Check the premises, then the inference. |

Rows marked **VERIFY** are the ones I am least sure a maintainer would accept. The final report repeats them.

Sources used (all fetched 2026-09-30):

* KMP: [Swift export](https://kotlinlang.org/docs/native-swift-export.html), [ObjC interop](https://kotlinlang.org/docs/native-objc-interop.html), [Kotlin/Wasm](https://kotlinlang.org/docs/wasm-overview.html), [Kotlin/JS](https://kotlinlang.org/docs/js-overview.html), [iOS integration](https://kotlinlang.org/docs/multiplatform/multiplatform-ios-integration-overview.html), [Google KMP page](https://developer.android.com/kotlin/multiplatform), [Compose Multiplatform](https://kotlinlang.org/compose-multiplatform/), [CMP FAQ](https://kotlinlang.org/docs/multiplatform/faq.html), [CMP iOS accessibility](https://kotlinlang.org/docs/multiplatform/compose-ios-accessibility.html), [Touchlab](https://touchlab.co/the-future-of-kmps-ios-interop).
* UniFFI: [README](https://raw.githubusercontent.com/mozilla/uniffi-rs/main/README.md), [Motivation](https://raw.githubusercontent.com/mozilla/uniffi-rs/main/docs/manual/src/Motivation.md), [lifting and lowering](https://raw.githubusercontent.com/mozilla/uniffi-rs/main/docs/manual/src/internals/lifting_and_lowering.md), [Kotlin configuration](https://raw.githubusercontent.com/mozilla/uniffi-rs/main/docs/manual/src/kotlin/configuration.md), [Kotlin template](https://raw.githubusercontent.com/mozilla/uniffi-rs/main/uniffi_bindgen/src/bindings/kotlin/templates/Helpers.kt), [futures](https://mozilla.github.io/uniffi-rs/latest/futures.html).
* Crux: [README](https://raw.githubusercontent.com/redbadger/crux/master/README.md) (master, read 2026-09-30), [effects chapter](https://redbadger.github.io/crux/part-2/effects.html), [Photoroom part 3](https://www.photoroom.com/inside-photoroom/building-live-collaboration-in-rust-for-millions-of-users-part-3).
* Shared UI: [Flutter architecture](https://docs.flutter.dev/resources/architectural-overview), [React Native render pipeline](https://reactnative.dev/architecture/render-pipeline), [React Native New Architecture](https://reactnative.dev/docs/the-new-architecture/landing-page).

## Post 1: Keel vs Kotlin Multiplatform

| ID | Section | Claim as written | Basis | Note |
|---|---|---|---|---|
| K01 | short version | KMP compiles Kotlin to each target: JVM and Android, native code for iOS, JavaScript or WebAssembly for the web | DOC (Kotlin/JS, Kotlin/Wasm, ObjC interop pages) + BG | |
| K02 | short version | Shared code runs inside a Kotlin runtime on every platform and platform code calls into it | DER | Architectural; true of every KMP target |
| K03 | short version, shares | KMP can share only logic, or logic and UI with Compose Multiplatform | DOC (CMP page: "one component, one screen, or the entire UI") | |
| K04 | shares, better-choice | CMP: iOS Stable, web Beta | DOC (CMP page status table) | Status as of the fetch; the post dates it |
| K05 | what Swift sees | On iOS, Kotlin/Native compiles a shared module into a framework that Swift reaches through an Objective-C header | DOC (ObjC interop) | |
| K06 | what Swift sees | Generics map to Objective-C lightweight generics "with limits" | DOC (ObjC interop, generics section) | |
| K07 | what Swift sees | Boxed primitives surface as `NSNumber` and `KotlinInt` | DOC (ObjC interop, nullability/boxing) | The page says primitive boxes map to `NSNumber` and that function-type parameters show as `KotlinInt` |
| K08 | what Swift sees | `suspend` functions appear as completion-handler functions, or as `async` in Swift with some limitations | DOC (ObjC interop) | Fetch summary says "experimental with limitations"; **VERIFY** current wording, it may have moved |
| K09 | what Swift sees | `Flow` has no Swift counterpart in that mapping, so teams commonly add community tooling such as SKIE or KMP-NativeCoroutines | DOC (Flow absent from the mapping page) + SRCH (SKIE, KMP-NativeCoroutines described in articles) + 3P (Touchlab) | "commonly" is a judgement; **VERIFY** |
| K10 | what Swift sees | Swift export generates Swift directly; its documentation calls it Alpha on a page last modified 28 August 2026; lists limitations including type-erased generics; says breaking changes are expected | DOC (Swift export, re-fetched to confirm the date and the two Alpha sentences) | |
| K11 | what Swift sees | In June 2026 Touchlab (maker of SKIE) still recommended SKIE for production iOS UIs while describing Swift export as the direction things are heading | 3P (Touchlab post dated June 12, 2026) | Touchlab's wording is stronger ("remains the definitive... choice"); we paraphrased softly |
| K12 | what Swift sees | As of September 2026 the mainstream path for a SwiftUI team is the ObjC framework, usually with a community plugin | DER (K05, K10, K11) | Judgement; **VERIFY** with a KMP maintainer |
| K13 | what Swift sees | On Android shared Kotlin is just Kotlin on Android's runtime, with no cross-language boundary | BG | Architectural |
| K14 | read path | On iOS, reads go through the bridged API: an in-process call into the Kotlin/Native runtime that ships inside the app, with no serialisation step | BG | **VERIFY**: "runtime ships inside your app" (Kotlin/Native memory manager is linked into the framework). An earlier draft also said "no copy of the value"; removed because Kotlin collections bridge to `NSArray` and the copy semantics are unclear |
| K15 | read path | For a `Flow`, Swift needs a collector that republishes values into something SwiftUI observes, which is what SKIE and similar libraries generate | BG + SRCH | **VERIFY** |
| K16 | web | Kotlin/JS compiles Kotlin to JavaScript | DOC (Kotlin/JS overview) | The page gives no stability status; the post does not state one |
| K17 | web, table | Kotlin/Wasm targets WebAssembly with garbage collection in the browser; its documentation still calls it Beta | DOC (Kotlin/Wasm overview: "still in Beta") | |
| K18 | tooling | JetBrains documents ways into Xcode: build-phase script (default with its IDE plugin), local Swift package, CocoaPods, prebuilt XCFramework via SwiftPM or CocoaPods | DOC (iOS integration overview) | |
| K19 | tooling | KMP is one integrated toolchain: Kotlin and Gradle, with JetBrains' IDE support | BG | |
| K20 | tooling | A team already running Gradle installs nothing new; the second toolchain lands on the Android team with KMP | DER | |
| K21 | ecosystem | Kotlin and KMP are developed by JetBrains | BG | Well known |
| K22 | ecosystem, table | Google's documentation states KMP is officially supported for sharing business logic between Android and iOS and is stable and production-ready | DOC (developer.android.com/kotlin/multiplatform) | |
| K23 | ecosystem | Google has made Jetpack libraries including ViewModel, lifecycle, DataStore, Room, Paging and SQLite KMP-ready | DOC (same page, library table) | |
| K24 | ecosystem | Ktor for HTTP; SQLDelight for type-safe Kotlin generated from SQL; Koin for dependency injection | SQLDelight: RAW (its README says "generates typesafe Kotlin APIs from your SQL statements"); Ktor, Koin: BG | The post deliberately says nothing about who maintains SQLDelight or Koin |
| K25 | ecosystem | With KMP you would typically assemble a data layer from HTTP and database libraries, often with a caching library on top, plus your own glue | BG | Judgement; no library is named |
| K26 | better-choice | KMP and Compose target desktop | DOC (CMP page: Desktop production-ready) | |
| K27 | better-choice | Kotlin on the server can share models with clients | BG | |
| K28 | better-choice | CMP offers interop with SwiftUI and UIKit where you want native parts | DOC (CMP page: "keep your existing SwiftUI, Android Views, or Swing code"; docs mention UIKit compatibility APIs) | |
| K29 | shares, better-choice | A shared relational database is available in KMP today (SQLDelight, Room) | DOC (Google page lists Room and SQLite as KMP-ready) + RAW (SQLDelight README lists iOS native, Android, JVM, JS) | |
| K30 | better-choice | "A long production record and two large companies behind it" | BG | **VERIFY**: KMP went stable in Nov 2023 (BG) but was in production use earlier |
| K31 | side by side | Runtime on iOS "Kotlin/Native, linked into the app" | BG (same as K14) | |
| K32 | side by side | Dev loop for KMP is "Gradle" | BG | Deliberately terse; we make no claim about KMP hot reload, build speed or IDE features |
| K33 | side by side | Android row: "Kotlin on Android's runtime, no boundary" | BG (same as K13) | |
| K34 | lede | (Negative claim about ourselves) we have not benchmarked KMP | n/a | `bench/RESULTS.md` lists the KMP and UniFFI comparison as not yet done |

## Post 2: Keel vs UniFFI and Crux

| ID | Section | Claim as written | Basis | Note |
|---|---|---|---|---|
| U01 | UniFFI | UniFFI is Mozilla's toolkit for generating foreign-language bindings for Rust libraries | RAW (README, docs home) | |
| U02 | UniFFI | You describe the interface with proc-macros or a UDL file, build a shared library, run the generator | RAW (README, Motivation) | |
| U03 | UniFFI | First-party support for Kotlin, Swift, Python (more limited Ruby); third-party bindings for JavaScript and WebAssembly, Kotlin Multiplatform, Go, C# | RAW (README; docs home says Ruby is limited) | |
| U04 | UniFFI, table, better-choice | Mozilla uses it extensively in Firefox on mobile and desktop; it describes UniFFI as ready for production but a long way from 1.0 | RAW (README) | |
| U05 | UniFFI | Simple values cross directly; strings, options and records are lowered into a byte buffer the project calls a `RustBuffer` and lifted on the other side | RAW (lifting and lowering chapter) | |
| U06 | UniFFI | The generated Kotlin loads the library through JNA | RAW (Kotlin template imports `com.sun.jna`; configuration page mentions JNA) | |
| U07 | UniFFI | It supports async functions, callback interfaces and foreign traits | DOC (futures page) | |
| U08 | UniFFI, table, hash gate | Its generated bindings check checksums when the library initialises | RAW for Kotlin (configuration option `omit_checksums`: "whether to omit checking the library checksums as the library is initialized") | **VERIFY** for Swift and Python; I assumed the same |
| U09 | UniFFI | Its own documentation says it offers no end-to-end packaging: it will not compile Rust for Android or package bindings into an `.aar` | RAW (Motivation, "Why Not?") | |
| U10 | UniFFI | UniFFI has no notion of observable state, a cache or a dev loop | DER (absence from the documentation; it is a bindings generator) | A maintainer would most likely agree; **VERIFY** |
| U11 | UniFFI | If a SwiftUI screen must stay in sync with Rust state you design the synchronisation (polling, or callback interfaces that push) | DER + BG | |
| U12 | UniFFI, table | Web: third-party bindings only | RAW (README lists uniffi-bindgen-react-native targeting WASM and React Native) | Our blueprint said "no first-class web target"; the post says "third-party bindings" instead |
| U13 | table | UniFFI generates "Swift and Kotlin APIs mirroring your Rust interface" | BG | |
| U14 | better-choice | UniFFI is smaller and simpler to adopt than anything that also brings a runtime | DER | Judgement |
| U15 | better-choice | Python and Ruby are in the tree; third-party generators cover Go, C# and more | RAW (README) | |
| U16 | better-choice | UniFFI is used extensively in Firefox and maintained by Mozilla | RAW (README) | |
| U17 | Crux | Crux is maintained by Red Badger | RAW (README: sponsors, "Red Badger Consulting Limited") | |
| U18 | Crux | Core in Rust, Shell in the platform's language; Elm architecture; `Event`, `Model`, `ViewModel`, `Effect`; `update` maps event and model to a `Command` | RAW (README) | |
| U19 | Crux, effects | The core never performs I/O; it describes the work as data and the shell performs it and hands the outcome back | RAW (README) + DOC (effects chapter) | |
| U20 | Crux | A shell is a loop: send an event, perform each requested effect and resolve it, read the view model when a render is requested | RAW (README, "Example Message Exchange Cycle") | |
| U21 | Crux, table | README states Crux is pre-1.0 and production-ready, with occasional breaking changes; links write-ups from Proton and Photoroom | RAW (README) | |
| U22 | Crux | Illustrative snippet: `Http::get(API_URL).expect_json().build().then_send(...)`, `render()`, `resp.take_body().unwrap()`, `impl App for Todos` | `Http::get...then_send` is DOC (effects chapter example); `render()`, `take_body`, the `Loaded(Ok(resp))` shape are BG | Labelled "illustrative ... not compilable as written". **VERIFY** if a reviewer objects to any API name |
| U23 | Crux | The boundary is three calls on a bridge: `update`, `resolve`, `view` | RAW (README) | |
| U24 | Crux | All messages are serialised with Bincode; types for Swift, Kotlin and TypeScript are generated with facet-generate; README names BoltFFI as the FFI generator | RAW (README) | Our own blueprint still says Crux sits on UniFFI; the README now names BoltFFI. Not mentioned in the post beyond the README statement |
| U25 | Crux | Published capabilities are Render, Http, KeyValue and Time | RAW (README) | The README says "a few capabilities at various stages of maturity"; others may exist outside the repo |
| U26 | Crux | Shells can be SwiftUI, Compose, React or Vue, or Rust web frameworks such as Leptos and Yew; a Rust shell can import the core directly | RAW (README) | |
| U27 | change-sets | When the core asks for a render, the shell calls `view()` and receives the whole view model, serialised across the boundary | RAW (README: `view: () -> ViewModel`, all messages serialised) + DER | **VERIFY** with Crux maintainers: the README does not say "whole" in so many words |
| U28 | change-sets | The shell must work out what changed, which SwiftUI and React already do by diffing | BG | |
| U29 | change-sets | The cost scales with the size of the view model, not the size of the change | DER | Not benchmarked; architectural inference |
| U30 | change-sets | Photoroom's engineering blog describes hitting this limit and building patches on top: key-path changes generated by diffing view models before and after an update | 3P (Photoroom part 3, fetch summary) | **VERIFY** wording; the post says "hitting exactly this limit", the article frames it as inefficiency of sending complete view models |
| U31 | effects | README highlights: the core stays sandboxed and portable (including in wasm), and tests play the role of the shell, resolving effects "with no fakes, mocks or stubs" | RAW (README, "Side-effect-free core", "Testing") | |
| U32 | effects | Commands can be written with async Rust, but the async block talks to the shell through a context rather than touching the model, so state changes live in `update` | DOC (effects chapter: `Command::new(|ctx| async move ...)`, context "without requiring model access") | |
| U33 | effects, table | A Crux app writes its own caching on top of the Http capability | DER (README lists no cache capability) | **VERIFY**; a maintainer may point at `crux_http` features or established patterns |
| U34 | effects | A Crux `update` is a pure function you can read top to bottom | DER (README: side-effect-free core) | Also a judgement on readability |
| U35 | table | Crux web: wasm core; React, Vue and Rust-based shells | RAW (README) | |
| U36 | table | Crux load-time check: "generated types that fail to compile on change" | RAW (README: type changes "cause type errors where appropriate") | |
| U37 | better-choice | Crux: Elm's discipline, small view models, Rust web shells, a community with production stories, no need for a data layer | DER + RAW (README) | Advocacy written in Crux's favour |
| U38 | three layers, table | UniFFI owns the crossing; Crux owns the shape of the app; "Keel does not build on UniFFI or Crux" | Keel side: own wire format and 19-function C ABI (`docs/SPEC.md` section 6) | |

## Post 3: Why Keel keeps your UI native

| ID | Section | Claim as written | Basis | Note |
|---|---|---|---|---|
| T01 | two bets | Flutter has its own rendering engine, Impeller, and its own implementations of each UI control rather than deferring to the system's | DOC (Flutter architectural overview) | |
| T02 | two bets | "You write Dart, and Flutter draws every pixel" | BG + DER | **VERIFY** "every pixel": platform views embed native UI in places; the overview says Flutter paints with its own engine |
| T03 | two bets | React Native turns a tree of React components into platform host views: `UIView` on iOS, `android.view.ViewGroup` or `android.widget.TextView` on Android | DOC (render pipeline) | |
| T04 | two bets | React Native's New Architecture has been the default since 0.76 | DOC (New Architecture landing page) | |
| T05 | two bets | React Native: "you write JavaScript or TypeScript" | BG | |
| T06 | two bets | CMP: iOS Stable, web Beta; interop lets you keep existing SwiftUI or Android View code | DOC (CMP page) | |
| T07 | two bets | CMP's FAQ says components are drawn on a canvas rather than being platform widgets | DOC (CMP FAQ: "all of the components are drawn on a canvas") | An earlier draft named Skiko; removed because the FAQ does not name it |
| T08 | two bets | KMP without CMP, UniFFI, Crux and Keel make the "share logic" bet | DER | |
| T09 | trades | React Native inherits much of the platform behaviour (text selection, scroll physics, navigation) because its screens are host views | DER (T03) + BG | **VERIFY**: "much of" is a hedge; navigation libraries vary |
| T10 | trades | Flutter and CMP draw their own controls, so matching a platform convention is the toolkit's work, or yours | DER (T01, T07) | |
| T11 | trades | Flutter documents platform views for embedding native UI, notes synchronisation overhead, and recommends them for complex controls where reimplementing is impractical | DOC (Flutter overview, platform views section) | |
| T12 | trades | When a platform ships a new UI API a shared toolkit adopts it on its own schedule, or you embed a native view | DER + BG | Architectural; no specific feature or date is cited |
| T13 | trades | Some surfaces, such as iOS widgets, are built in the platform's own UI framework regardless, so a shared-UI app writes those natively anyway | BG | **VERIFY**: WidgetKit widgets are SwiftUI-only; RN and Flutter apps add widgets as native extensions |
| T14 | trades | Flutter documents a semantics tree separate from its render tree | DOC (Flutter overview) | |
| T15 | trades | JetBrains documents that Compose semantics are mapped to native iOS accessibility objects and support VoiceOver | DOC (CMP iOS accessibility page) | |
| T16 | trades | "These work and are widely used. The cost is that keeping parity ... is the toolkit's ongoing job" | DER | Judgement; no claim that any toolkit is inaccessible |
| T17 | trades | Platform teams hire SwiftUI, Compose and React specialists; Dart, React Native and Kotlin-with-Compose pools are each large | BG | Opinion |
| T18 | buys | These toolkits put a lot of effort into fast feedback loops; a React team can reach mobile through React Native; a Kotlin team can reach iOS through CMP | BG + DOC (CMP) | |
| T19 | why not | Flutter, React Native and CMP are whole-UI products each maintained by a large team that chases platform UI features, keeps accessibility in step and polishes rendering | BG | Opinion; maintainers are Google, Meta, JetBrains |
| T20 | choosing | Table rows: React Native for an existing React team; CMP (iOS Stable) for a Kotlin-first team | DER (T04, T06) | |

## Post 4: Reads never cross the boundary

Nearly every claim in this post is about Keel and is sourced in the appendix. The only claims about other tools:

| ID | Section | Claim as written | Basis | Note |
|---|---|---|---|---|
| A01 | the rule | There are roughly three ways to keep native UI in step with state in another language (per-value reads, whole-state sends, a local copy plus deltas) | DER | Generic framing; no tool is named in that paragraph |
| A02 | schema hash | "Other tools have comparable load-time checks, and we do not claim this one is unique" | U08 (UniFFI checksums) | |
| A03 | the rule | A link to the UniFFI and Crux comparison "looks at how other tools place themselves" | n/a | |

## Appendix: Keel-side claims a second reader should check

These are not claims about other tools, but the comparison posts lean on them.

| Claim | Where it comes from |
|---|---|
| 49.8 ns synchronous call through the C ABI (core side); 31.5 ns without the ABI allocation; 43.9 ns `dispatch/call_sync/add` (not quoted in a post) | `bench/RESULTS.md`, "C ABI" and "Dispatch" tables |
| 1 KB record round trip 228.0 ns; 100-signal change-set 2.30 µs core side, 253.9 ns decode; cold start 70.87 µs | `bench/RESULTS.md`, "Section 14 rows", "Signals and stores", "Snapshot and restore" |
| Keyed insert into 10,000 rows 6.31 µs recorded vs 536.16 µs diffed; update 272.3 ns vs 532.32 µs; move 9.74 µs vs 702.95 µs; 85 percent of the old cost was the diff; raw update 528.7 µs in the gate harness; 2.57 µs decode-and-replay in the Rust wire crate | `bench/RESULTS.md`, Finding 1 and "Wire: keyed patch" |
| 85 KB gzipped wasm (runtime plus hello-world core, budget 120 KB); 831 KB Android core per ABI | `README.md`, "Why it's fast" |
| 46 gated operations, budgets about 5x what they measure | `bench/RESULTS.md`, "The CI gate" |
| More than 3,800 tests across five languages; 17 scenarios on three platforms, 51 of 51 | `README.md`, "Why you can trust it" |
| Scenario behaviours: S09 (three entries, one transaction), S10 (one `Insert` under 100 bytes), S11 (1,000 reads, no crossing), S12 (31 s clock advance), S16 (schema mismatch), S17 (panic containment native and wasm) | `contract-tests/scenarios.md` |
| Generated Swift, Kotlin, TypeScript snippets | `examples/playground/generated/*`, abbreviated with `// ...` where marked; the Swift `apply` excerpt is verbatim |
| Playground UI snippets | `examples/playground/ios/.../TodosScreen.swift`, `android/.../TodosScreen.kt`, `web/src/views/TodosView.tsx` (abridged) |
| Generated Swift wrappers stop the process (`keelUnexpected`, a `fatalError`) on a core panic | `examples/playground/generated/swift/.../Errors.swift` (comment on `keelUnexpected`). Kotlin and TypeScript throw or reject. This sits oddly with R6's wording ("nothing escapes as a panic or an abort") and is stated plainly in post 4 |
| `keel dev` has no Android remote transport yet | `site/docs/cli.html` ("the Kotlin runtime has no remote transport on Android yet") |
| Keel v1 has no sync engine, no Rust-owned SQLite, no shared UI; no desktop targets yet | `README.md` ("Not in v1"), `docs/SPEC.md` section 0 (out of scope: desktop beyond macOS-via-Swift, Rust-owned SQLite) |
| Observed keyed list exists three times (core list, core baseline, platform mirror) | `.10x/adrs/ADR-027-keyed-list-ops-are-recorded-not-diffed.md` ("baseline stays"), `docs/SPEC.md` section 16.1 |
| `Computed<Vec<T>>` crosses as a full value (only a keyed `Signal<Vec<T>>` gets patches) | `docs/SPEC.md` section 4.3 and E0008; generated `visible` handles only `.fullValue` |
| Platform runtimes apply change-sets in one main-thread hop per batch | `runtimes/swift/.../Mirror.swift`, SPEC section 11 (the docs page says "per-frame coalescing"; the code batches per hop, and the posts say "in one hop") |

## Discrepancies found in our own material (not in the posts)

* `docs/blueprint.html` section 02 describes Crux as "UniFFI underneath" and as re-emitting the whole view model on every event. Crux's README (read 2026-09-30) names BoltFFI, and the view is pulled with `view()` after a render request. The posts follow the README.
* The blueprint says UniFFI costs "serialization on every call". UniFFI's documentation says simple values cross directly and only non-trivial types are lowered to a byte buffer. The posts follow UniFFI's documentation.
* The blueprint says React Native is "a bridge" and Flutter/RN are "own renderer / bridge". React Native's documentation says it mounts host views. The post follows the documentation.
* The blueprint's "This is what KMP cannot do" (dev loop) was not reused; it is a claim we could not substantiate.
* `README.md` lists 897 TypeScript tests; `.10x/status.md` lists 869. The posts quote only the README's "3,800+" total.
* `site/docs/concepts.html` says change-sets are applied "with per-frame coalescing"; the Swift and TypeScript runtimes batch per main-thread hop or macrotask. The posts avoid "per-frame".
* `README.md` labels the 49.8 ns row "C ABI, end to end"; `bench/RESULTS.md` says it is the core half only. The posts use the latter wording.
