# Claims ledger: "Why Undra is the default choice"

Post: `site/blog/why-undra-is-the-default-choice/index.html`, published 2026-10-02. Piece: `default-choice-post` (H4).
Written by the post's author on 2026-10-02 against `main` `d1b35b5`; **fact-checked adversarially on 2026-10-02** against `main` `da4fbbe`
(every code piece of the v1.x program merged, `.10x/status.md` checkpoint 26), merged into the branch first. The pass is recorded in
`.10x/reviews/2026-10-02-default-choice-post-fact-check.md`. Same purpose as `.10x/reviews/2026-09-30-blog-claims.md`: anyone can check
every factual sentence of the post quickly and independently.

## How to read the columns

**Aliases for paths.** `cat` = `.10x/specs/2026-10-01-competitive-limitations.md` (`cat:173` is a line of it; a source id such as K2 or U5 is
its section 12, which has the URL). `res` = `bench/RESULTS.md`. `adr-NNN` = `.10x/adrs/ADR-NNN-*.md`. `rev:x` = `.10x/reviews/*-x-review.md`.
`stat` = `.10x/status.md`. `sde:x` = `.10x/decisions/sde/x.md`. `design` = `.10x/specs/2026-10-01-v1x-default-choice-design.md`. `SPEC` =
`docs/SPEC.md`. `sc` = `contract-tests/scenarios.md`. `q` = `crates/undra-query/src/lib.rs`. `ledger-0930` = `.10x/reviews/2026-09-30-blog-claims.md`.
Site pages are named by file (`db.html` is `site/docs/db.html`, `from-kmp.html` is `site/docs/cookbook/from-kmp.html`). Line numbers are of the
merged tree (`main` `da4fbbe` plus this branch).

**Basis**, the catalogue's codes: RAW (a raw file: README, source template), DOC (an official documentation page), 3P (a third-party article),
REPO (this repository), DER (judgement or inference, or a reading of a page's silence; the post words those "as we read" or "its documentation
describes no").

**Checked by.** `A` = the author opened the cited repository path. `A-cat` = the author carried the claim from the catalogue. `A-judge` = the
author's judgement.

**Fact-check** (the last column). How the competitor pages were read: every URL the post links was fetched again on **2026-10-02** with `curl`
(the raw HTML or file, converted to text locally and searched for the claim's words; no summarising fetch), and the quote in the row is from that
copy. For UniFFI's silence, all 68 pages of its manual (`docs/manual/src/**/*.md`, listed through the GitHub tree API) and its README were
fetched and searched. Verdicts:

* **verified** `<where>`: the source says what the post says, and the number is exact.
* **corrected**: the post's sentence was wrong, stale or rounded in its own favour, and was changed in this pass. The "Claim as written" column
  has the new wording; the old one follows "was:".
* **removed**: the claim is no longer in the post; the reason follows.
* **added**: a claim the fact-check put into the post (refreshing what moved), verified the same way.

## 1. Lede and "How to read this page"

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| P01 | A team choosing how to share logic across iOS, Android and the web asks "what will this stop me doing?" first | design:8-10 (the bar: "no engineer declines Undra because of something it cannot do") | DER | A-judge | verified (framing) design:8-10 |
| P02 | We catalogued 68 limitations of KMP, UniFFI, Crux, React Native, Flutter, Capacitor and writing the logic three times | cat:51-60 (18 + 15 + 9 + 9 + 9 + 3 + 5 = 68) | REPO | A | verified cat:51-60 |
| P03 | each dated and sourced where a source exists | cat:85-91; cat:353-355 (the "write three times" rows have no primary source) | REPO | A | verified cat:85-91, 353-355 |
| P04 | and mapped every one to what Undra's repository does about it | cat:48-60 (SOLVED / PLANNED / MISSING / N/A per alternative) | REPO | A | verified cat:48-60 |
| P05 | Each statement about another tool links to the page it comes from, and we read every one of those pages again on 2 October 2026; a reading of a page's silence is as of the same date. *Was: "Statements about other tools are as of 1 October 2026"* | this pass's re-fetch (the method above) | REPO | fact-check | **corrected**: the pages were re-read; the date is the re-read's |
| P06 | Timings are measured on an Apple M5 Pro host, its iPhone simulator, its Android emulator or headless Chromium | res:16-24; res:874, :898, :922 | REPO | A | verified res:16-24, 874, 898, 922 |
| P07 | none is from a physical device | res:965-970 (Pending hardware); res:1034-1038 | REPO | A | verified res:965-970 |
| P08 | the claims ledger and the catalogue are linked | the two GitHub links resolve to this file and to `cat` once merged; the post's footer links this file as deployed (`claims.md`, which `site/scripts/stage.sh` copies with `site/`) | REPO | A | verified (both files exist; `check-links.mjs` passes) |
| P09 | none compares Undra's speed with another tool's | the post: every timing is Undra's own | REPO | fact-check | **added**; verified by reading the post |

## 2. The matrix: introduction

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| MI01 | The catalogue's matrix lists 36 things a team needs | cat:380-417 | REPO | A | verified cat:380-417 |
| MI02 | On 1 October Undra was marked as having 10 of them, part of 9 and none of 16, with one not applicable | cat:382-417, "Undra today" column, tallied in section 11 | REPO | A | verified, re-tallied: yes = rows 1, 2, 3, 5, 6, 7, 19, 20, 21, 25; part. = 4, 8, 14, 15, 23, 26, 27, 31, 34; no = 9-13, 16-18, 22, 24, 28-30, 32, 33, 35; n/a = 36. Row 2 ("yes native · part. web") counts as yes, which flatters the 1 October baseline, not today |
| MI03 | Since then every code piece of the v1.x program has merged, each feature after an adversarial review recorded in the repository. *Was: "Since then the pieces below have merged, each after an adversarial review"* | stat:512 ("Every code piece of the v1.1/v1.2 program is merged"); stat:305-512, one review per feature row | REPO | A | **corrected**: two small fix pieces merged without a review (swift-fs, stat:400; dev-reload-flake, stat:462), so "each" is scoped to features |
| MI07 | At the last merge the contract grid passed 95 of 95 cells over 33 scenarios | stat:511 ("contracts 95/95 (S01–S33)"); sc:561, :578 (S21 and S22 are TypeScript only: 31 × 3 + 2 = 95) | REPO | fact-check | **added**; verified stat:511 |
| MI08 | the test suites ran 3,536 Rust, 1,856 TypeScript, 881 Kotlin, 870 Swift and 110 React Native tests | stat:511 ("Rust 3,536 · TS 1,856 + 37 · Kotlin 881 + 32 · Swift 870 · RN 110"; the +37 and +32 are separate suites the post does not count) | REPO | fact-check | **added**; verified stat:511 |
| MI04 | Counting again today: 27 solved, 6 partial, 2 open and 1 left out by decision. *Was: 22 solved, 7 partial, 6 open* | section 11, re-derived row by row against `main` `da4fbbe` | DER | A-judge | **corrected**: six rows moved with what merged after the draft (T10, T13, T17, T22, T23, T33) |
| MI05 | The table groups the 36 rows into 12 classes | the row labels cover 1-36 once each: 3-6, 1-2, 21-23, 7-10, 11/12/14, 15-16, 18-19, 20/24/30, 25-29 + 36, 31-32, 13/17/33/34, 35 | REPO | A | verified (counted: 4+2+3+4+3+2+2+3+6+2+4+1 = 36) |
| MI06 | It leaves out Crux, which its own post covers | `site/blog/undra-vs-uniffi-and-crux/index.html` | REPO | A | verified |

## 3. The matrix: one block per class

`K` = Kotlin Multiplatform, `U` = UniFFI, `F` = Flutter / React Native, `N` = Undra. Every K, U and F row was re-read on 2026-10-02 (method above).

### Class 1: State reaches the UI (rows 3 to 6)

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| M01-K1 | On iOS Swift copies a Kotlin collection whole | K2 kotlinlang.org/docs/native-objc-interop.html | DOC | A-cat | verified K2: "the Swift compiler copies the entire collection" |
| M01-K2 | observing a `Flow` takes SKIE or Swift export, which is Alpha. *Was: "takes SKIE or similar"* | K1 kotlinlang.org/docs/native-swift-export.html; K16 skie.touchlab.co | DOC | A-cat | **corrected**: K1 says Swift export can "export kotlinx.coroutines flows as Swift's AsyncSequence out of the box" and is "currently in Alpha"; K16: "Use any Flow as an AsyncSequence from Swift". The draft left out Swift export, as the 2026-09-30 fact-check (its M2) had warned against |
| M01-U1 | UniFFI's documentation describes no observable state | U1, U2 and the whole manual | DER | A-cat | verified (silence, 2026-10-02): "observ" appears in none of the 68 manual pages or the README |
| M01-U2 | reading Rust state is a call across the boundary | U7 lifting_and_lowering.md | DER | A-cat | verified (inference from U7: "Non-trivial types such as Strings, Optionals and Records, etc. are lowered to a byte buffer", plus M01-U1) |
| M01-F1 | Flutter / React Native: no language boundary | the post's own reading of the column (logic in the UI's language); cat:422 footnote 1 | DER | A-cat | verified (by the definition the post states) |
| M01-F2 | React Native runs logic on the JS thread; a frame it misses is dropped | R3 reactnative.dev/docs/performance | DOC | A-cat | verified R3: "your business logic will run on the JavaScript thread"; "If the JavaScript thread is unresponsive for a frame, it will be considered a dropped frame" |
| M01-N1 | Undra: reads are local | CLAUDE.md:16 (R5); sc:267-268 (S11 step 6: 1,000 reads, `crossings.calls` unchanged) | REPO | A | verified CLAUDE.md:16, sc:267-268 |
| M01-N2 | a write crosses once | CLAUDE.md:16; sc:225-237 (S09) | REPO | A | verified sc:225-237 |
| M01-N3 | lists, and filtered or sorted views of them, cross as O(change) | adr-027; adr-039; res:340-376 (finding 1), res:442-457 (finding 5); sc:239 (S10), :499-528 (S19) | REPO | A | verified (wording extended to name the derived views ADR-039 added) |
| M01-N4 | delivery is merged per frame | adr-031:1; sc:478 (S18); res:947-951 | REPO | A | verified adr-031:1, sc:478 |

### Class 2: Errors and crashes (rows 1, 2)

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| M02-K1 | Only `@Throws` exceptions become Swift errors; others end the program | K2 | DOC | A-cat | verified K2: "Other Kotlin exceptions reaching Swift/Objective-C are considered unhandled and cause program termination" |
| M02-U1 | As we read its Swift templates, a Rust panic in a non-`Result` function stops the process | U5: `macros.swift` (`is_try`: `try` if the function throws, else `try!`), `Helpers.swift` (`throw UniffiInternalError.rustPanic(...)`) | RAW / DER | A-cat | verified both templates on `main`; the link now points at `macros.swift`, where the `try!` is (it pointed at `Helpers.swift`) |
| M02-F1 | Flutter swaps a failed widget for a gray background in release | F10 docs.flutter.dev/testing/errors | DOC | A-cat | verified F10: "in release mode this shows a gray background" |
| M02-F2 | Platform channels answer with a code and a message. *Was: "Native modules send codes and messages"* | F2 docs.flutter.dev/platform-integration/platform-channels | DOC | A-cat | **corrected**: the linked page is Flutter's (`result.error("UNAVAILABLE", "Battery level not available.", null)`); "native modules" is React Native's term |
| M02-N1 | Undra: one closed `UndraCallError` set on every platform | docs/ERRORS.md:21-29 ("a closed set of five cases"), :11-19 | REPO | A | verified docs/ERRORS.md:11-29 |
| M02-N2 | panics are caught at the boundary | docs/ERRORS.md:26; sc:417 (S17) | REPO | A | verified |
| M02-N4 | and reach the app as a structured report | adr-046 amendment items 1-3; stat:491; sc:795 (S29; the React Native column skips it, adr-046 amendment 19, because it runs a wasm stand-in) | REPO | fact-check | **added**; verified adr-046 (Accepted), stat:491 |
| M02-N3 | a trapped wasm core restarts from its snapshot | adr-049; SPEC:646; sc:578 (S22); web.html | REPO | A | verified SPEC:646, sc:578 |

### Class 3: What can cross (rows 21 to 23)

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| M03-K1 | Swift sees an Objective-C header, or Swift export (Alpha), which erases generics | K1, K2 | DOC | A-cat | verified K1: "Kotlin generic type parameters are type-erased to their upper bounds"; "currently in Alpha" |
| M03-U1 | UniFFI: objects and foreign traits cross | U8 mozilla.github.io/uniffi-rs/latest/types/interfaces.html | DOC | A-cat | verified U8: "They can be freely passed as arguments or returned as values"; `#[uniffi::export(with_foreign)]` |
| M03-U2 | its documentation describes no generic types of your own. *Was: "its documented types list no generics"* | U13 builtin_types.md; the whole manual | DER | A-cat | **corrected**: U13's table does list generic containers (`Vec<T>`, `Option<T>`, `HashMap<K, V>`), so "no generics" was wrong as worded; no manual page documents a generic type of your own ("generic" appears only in three internals pages) |
| M03-F1 | `flutter_rust_bridge` takes arbitrary types and closures | F12 README | RAW | A-cat | verified F12: "(even with arbitrary types, closure, `&mut`, async, traits, etc)" |
| M03-F2 | React Native native modules need a spec and code per platform | R5 turbo-native-modules-introduction | DOC | A-cat | verified R5: "define a typed JavaScript specification"; "write your native platform code using the generated interfaces" |
| M03-N1 | Undra: records, enums and errors become native sum types | SPEC §10.1-10.3 (:753-886); docs/ERRORS.md:11-19 | REPO | A | verified |
| M03-N2 | no Objective-C | SPEC §10.1 (:753); cat:123 (E1) | REPO | A | verified |
| M03-N3 | Solved: objects cross as parameters and returns; the core calls host callbacks; newtypes stay typed. Partly: generics, one named type per instantiation, and no generic functions or objects. *Was: "Not yet: objects, callbacks, generics, newtypes"* | adr-040, adr-041, adr-042 (Status: Accepted, line 3); stat:482 (objects-callbacks), :500 (types-paging), :509 (follow-ups); SPEC:887 (§10.3a), :942 (§10.3b), :1051 (E0002 now names the instantiation route), :1079 (E0064: an object crosses as `Arc<T>` or `&T`); types.html:96-118; sc:705 (S27), :748 (S28), :861 (S31) | REPO | fact-check | **corrected**: all three ADRs shipped after the draft; generics are partial (types.html:118: "Generic objects, stores, functions, methods, ports, callbacks and queries stay out") |

### Class 4: Data layer (rows 7 to 10)

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| M04-K1 | KMP: libraries Room, SQLDelight, Store | K12 developer.android.com/kotlin/multiplatform; K19; K18 | DOC / RAW | A-cat | verified: K12's KMP-ready table lists `room`; K19 is SQLDelight's README; K18 is Store5's |
| M04-K2 | SQLDelight has compile-time migration checks | K19 | RAW | A-cat | verified K19: "It verifies your schema, statements, and migrations at compile-time" |
| M04-U1 | UniFFI's documentation describes no cache, mutation, offline or persistence layer | U1, U2, the whole manual | DER | A-cat | verified (silence, 2026-10-02): "cache", "mutation", "offline" and "persist" appear in none of the 68 manual pages or the README |
| M04-F1 | Flutter / React Native: libraries, TanStack Query | R18 | DOC | A-cat | verified R18 (network mode: a query "is paused until you have connection again") |
| M04-F2 | drift (with migrations) | F13 pub.dev/packages/drift | DOC | A-cat | verified F13: "builtin support for transactions, schema migrations" |
| M04-N1 | Undra: cache, dedup, retry, optimistic rollback | q:27-33, :69-80; sc:270 (S12), :290 (S13); queries.html | REPO | A | verified q:27-33, 69-80 |
| M04-N2 | an offline queue that survives a restart | sc:308 (S14); q:76-87; offline.html | REPO | A | verified |
| M04-N3 | migrating persisted state | adr-037 (Accepted); SPEC:173 (§2.5), :378 (§4.5a); rev:persistence | REPO | A | verified |
| M04-N4 | Not yet: optimistic placeholders after a restart | q:87-89 ("after a restart the replay invalidates just the mutation's own key, and there is no optimistic update left to roll back") | REPO | A | verified q:87-89 |
| M04-N5 | Solved: infinite queries, lazy lists. *Was: "Not yet: paged queries"* | adr-043 (Accepted, line 3); q:47-56; sc:889 (S32); paging.html; stat:500 | REPO | fact-check | **corrected**: ADR-043 shipped after the draft |
| M04-N6 | Solved: polling. *Was: "Not yet: polling"* | q:37-46; sc:918 (S33); polling.html | REPO | fact-check | **corrected**: the draft's source line ("timed refetching is not in the v1 contract") is gone from q |

### Class 5: Storage, sockets, services (rows 11, 12, 14)

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| M05-K1 | KMP: Ktor, DataStore, Room, SQLDelight; many of Google's Jetpack libraries are KMP-ready. *Was: "Google's Jetpack libraries are KMP-ready"* | K12; ktor.io/docs/client-engines.html; K19 | DOC | A-cat | **corrected**: K12 says "Many of our Jetpack libraries have already been migrated to be KMP-ready" (WorkManager is not in its table). Ktor: "The Ktor HTTP client is multiplatform"; DataStore and Room are in K12's table |
| M05-U1 | UniFFI's documentation describes no adapters; SQL and sockets are Rust crates you bring | the whole manual; cat:432-433 | DER | A-cat | verified (silence, 2026-10-02): "adapter", "sqlite" and "websocket" appear in none of the 68 manual pages |
| M05-F1 | React Native has WebSocket and `expo-sqlite` | R13, R15 | DOC | A-cat | verified R13: "React Native also supports WebSockets"; R15: expo-sqlite on Android, iOS, macOS, tvOS, Web |
| M05-F2 | Flutter has `drift` and `web_socket_channel` | F13, F14 | DOC | A-cat | verified F14: "cross-platform StreamChannel wrappers for WebSocket connections" |
| M05-N1 | Undra: default adapters for HTTP, storage, files and connectivity on iOS, Android, web and React Native | rev:android-adapters; stat:351 (checkpoint 8), :399 (checkpoint 14, React Native: all ten ports); ports.html | REPO | A | verified stat:351, 399 |
| M05-N2 | opt-in WebSocket, SSE and SQL ports | adr-047, adr-048 (Accepted); stat:453; sc:602, :627, :646 (S23-S25); realtime.html; db.html | REPO | A | verified |
| M05-N3 | Not yet: the web database serves one tab per origin | db.html ("which one tab holds at a time: a second tab of the same app gets `Unavailable`"); stat:453 ("Open: ... the web Db serves one tab per origin") | REPO | A | verified; no later checkpoint (stat:468-512) closes it |

### Class 6: Dev loop and tooling (rows 15, 16)

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| M06-K1 | Compose Hot Reload needs a desktop JVM target. *Was: "is desktop JVM only"* | K6 kotlinlang.org/docs/multiplatform/compose-hot-reload.html | DOC | A-cat | **corrected**: K6 says it "requires ... a JVM target" and "While we explore adding support for other targets, you can already use the desktop app as your sandbox"; it never says "only" |
| M06-K2 | JetBrains' 2025 roadmap calls build speed "a common Kotlin/Native concern". *Was: "Kotlin/Native build speed is a standing priority", linked to the compile-time guide* | K8 blog.jetbrains.com/kotlin/2025/08/kmp-roadmap-aug-2025/ | DOC | A-cat | **corrected**: the linked guide (K5) does not say that; K8 does: "Build speed remains a common Kotlin/Native concern". The link now goes to K8, dated in the sentence |
| M06-U1 | UniFFI: each change rebuilds the library and the app; its documentation describes no dev server or devtools | the whole manual | DER | A-cat | verified (silence, 2026-10-02): "hot reload", "live reload", "dev server", "devtools" and "inspector" appear in none of the 68 pages; the rebuild is the inference |
| M06-F1 | Both ship DevTools | R12, F8 | DOC | A-cat | verified (both pages exist and describe them) |
| M06-F2 | Flutter hot reload skips native code | F3 | DOC | A-cat | verified F3: "If you've changed native code (such as Kotlin, Java, Swift, or Objective-C), you must perform a full restart" |
| M06-F3 | React Native Fast Refresh can fall back to a full reload | R8 | DOC | A-cat | verified R8: "Fast Refresh will fall back to doing a full reload" |
| M06-N1 | Undra: `undra dev` reconnects | adr-051:35-36; cli.html; rev:dev-loop | REPO | A | verified |
| M06-N2 | keeps state across a rebuild | adr-053; docs/DEV_LOOP.md:88 | REPO | A | verified |
| M06-N3 | devtools with time travel | adr-054; docs/DEV_LOOP.md:225-242 | REPO | A | verified |
| M06-N4 | Gradle, Xcode and Vite run `undra build` | rev:tooling; stat:375; cli.html | REPO | A | verified stat:375 |
| M06-N5 | Not yet: query handles across a reload | stat:391 ("query handles do not survive a reload (needs its own ADR)"); docs/DEV_LOOP.md:254 | REPO | A | verified; still open at stat:512 |

### Class 7: Testing and previews (rows 18, 19)

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| M07-K1 | `kotlinx-coroutines-test` gives virtual time in multiplatform tests | K21 | DOC | A-cat | verified K21: "TestCoroutineScheduler The shared source of virtual time"; targets common, js, jvm, native, wasmJs |
| M07-U1 | UniFFI's documentation describes no fakes for platform I/O; you build the doubles | the whole manual | DER | A-cat | verified (silence, 2026-10-02): the manual mentions only mocks of the generated objects, which you write (`swift/overview.md:10`, `types/interfaces.md:62`), and no fakes for platform I/O |
| M07-F1 | Flutter's widget previewer supports no `dart:ffi` or native plugins | F9 | DOC | A-cat | verified F9: "Native plugins and any APIs from the dart:io or dart:ffi libraries are not supported" |
| M07-N1 | Undra: `PreviewCore` runs your real core with fakes and a manual clock on Swift, Kotlin and TypeScript | docs/TESTING.md:5-17; adr-055; rev:testkit | REPO | A | verified docs/TESTING.md:5-17 |
| M07-N2 | a recorded session replays in tests | docs/TESTING.md:8-9; SPEC:1712 | REPO | A | verified |

### Class 8: Compatibility and floors (rows 20, 24, 30)

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| M08-K1 | Kotlin/Native targets iOS 15.0 and later. *Was: "iOS 15.0 default minimum"* | K15 kotlinlang.org/docs/native-target-support.html | DOC | A-cat | **corrected**: K15 lists "Apple iOS and iPadOS 15.0 and later on ARM64 platforms"; it does not say "default" |
| M08-K2 | several KMP frameworks in one app duplicate dependencies | K7 | DOC | A-cat | verified K7: "the Kotlin/Native compiler duplicates the dependencies"; "any state passed by different modules through the same dependency won't be connected" |
| M08-U1 | As we read its Swift template, a library and bindings that disagree stop the process | `wrapper.swift` on `main` | RAW | A-cat | verified: `fatalError("UniFFI API checksum mismatch: try cleaning and rebuilding your project")` |
| M08-F1 | React Native's minimum is iOS 15.1 | R16 | DOC | A-cat | verified R16: "[0.76] iOS minimum OS version bump to 15.1" (7 Aug 2024) |
| M08-F2 | Flutter: iOS 15 | F7 | DOC | A-cat | verified F7: iOS "Supported 15 to 27" (Flutter 3.47) |
| M08-F3 | Flutter does not support several Flutter libraries in one app | F4 | DOC | A-cat | verified F4, under "Mobile limitations": "Packing multiple Flutter libraries into an application isn't supported" |
| M08-N1 | Undra: schema-hash gate at load | SPEC:151 (§2.3); sc:396 (S16) | REPO | A | verified |
| M08-N2 | iOS 15 and 16 via `ObservableObject` | adr-045 (Accepted); stat:463; ios-15-16.html | REPO | A | verified stat:463 |
| M08-N3 | several cores side by side | adr-044 (Accepted); stat:417; sc:671 (S26); several-cores.html | REPO | A | verified stat:417, sc:671 |
| M08-N4 | Caveat: the iOS 15 and 16 mode has not run on an iOS 15 or 16 runtime | stat:463 ("proven by compilation and a 26.5 runtime probe, no 15/16 runtime installed") | REPO | A | verified stat:463 |

### Class 9: Platforms and UI frameworks (rows 25 to 29, 36)

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| M09-K1 | Kotlin/Wasm is Beta | K9 | DOC | A-cat | verified K9: "Although Kotlin/Wasm is still in Beta" |
| M09-K2 | Compose Multiplatform shares UI; desktop is Stable | K11 | DOC | A-cat | verified K11: "Compose Multiplatform is Stable for Android, iOS, and desktop" |
| M09-U1 | UniFFI: Python and Ruby are first-party | U1 | RAW | A-cat | verified U1: "UniFFI comes with support for Kotlin, Swift, Python and Ruby" |
| M09-U2 | web comes from a third-party generator | U1 | RAW | A-cat | verified U1, "Third-party foreign language bindings": "Javascript bindings ... running in a web page, targeting WASM" |
| M09-F1 | Flutter paints one UI on six platforms | F7 | DOC | A-cat | verified F7: Android, iOS, Windows, macOS, Linux, web |
| M09-F2 | Flutter is not suited to text-rich static sites | F5 | DOC | A-cat | verified F5: "Flutter is not suitable for static websites with text-rich flow-based content" |
| M09-F3 | React Native: web, Windows and macOS out of tree | R9 | DOC | A-cat | verified R9: React Native macOS and Windows "From Partners", React Native Web "From Community" |
| M09-N1 | Undra: web (wasm core; React, Vue, Svelte, Solid hooks) | `runtimes/ts/@undra/runtime/src/{react,vue,svelte,solid}.ts`; web.html | REPO | A | verified |
| M09-N2 | React Native | adr-038; docs/REACT_NATIVE.md; stat:343, :399 | REPO | A | verified |
| M09-N3 | Not yet: Flutter and Dart | no Dart piece in stat:300-512; `site/data/roadmap.json` ("Flutter and Dart", Next) | REPO | A | verified |
| M09-N4 | Partly: desktop, and reuse on a server. *Was: "Partly: desktop"* | cat:152 (E30); roadmap.json ("Desktop targets", Later); row 27 (T27) | REPO | fact-check | **corrected**: row 27 (a server, partial) was in the class but unnamed in the cell |
| M09-N5 | By decision: no shared UI | README.md:191-192; roadmap.json (notPlanned) | REPO | A | verified |

### Class 10: Adopting and building (rows 31, 32)

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| M10-K1 | KMP: several ways into Xcode; its IDE plugin applies a build-phase script by default | K14 | DOC | A-cat | verified K14: "If you use the Kotlin Multiplatform IDE plugin, direct integration is applied by default"; direct integration is "a special script ... integrated into the build phase" |
| M10-U1 | UniFFI: no end-to-end packaging: no Rust for Android, no `.aar` | U2 Motivation.md | RAW | A-cat | verified U2: "UniFFI doesn't provide an end-to-end packaging solution"; "compiling the Rust code to run on Android or with packaging the bindings into an `.aar`" |
| M10-F1 | React Native joins an iOS app through CocoaPods | R10 | DOC | A-cat | verified R10: CocoaPods: "We use it to add the actual React Native framework code locally into your current project" |
| M10-F2 | Flutter add-to-app has limits | F4 | DOC | A-cat | verified F4 ("Limitations" section) |
| M10-N1 | Undra: `undra build` runs inside Gradle, Xcode and Vite | rev:tooling; stat:375 | REPO | A | verified |
| M10-N2 | SwiftPM on native iOS, no CocoaPods | cat:132 (E10); cli.html (bindgen writes a Swift package); docs/REACT_NATIVE.md:51 (React Native's iOS install uses the app's Podfile, hence "native iOS") | REPO | A | verified |
| M10-N3 | Partly: `undra adopt` edits none of your project files | `crates/undra-cli/src/commands/adopt.rs:3-7`; from-kmp.html | REPO | A | verified adopt.rs:3-7 |

### Class 11: Running in production (rows 13, 17, 33, 34)

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| M11-K1 | KMP: background work is platform code (WorkManager is Android-only) | X2 (Android's WorkManager page); K12 | DOC / DER | A-cat | verified: X2 is Android documentation and K12's KMP-ready Jetpack table (2026-10-02) has no `work` library; "background work is platform code" stays the catalogue's reading (cat:184) |
| M11-K2 | the IDE plugin debugs across Swift and Kotlin | K13 | DOC | A-cat | verified K13: "cross-language navigation and debugging for Swift and Kotlin" |
| M11-U1 | UniFFI: not assessed | cat:394, 398, 414, 415 | REPO | A | verified |
| M11-F1 | React Native Headless JS is Android-only | R11 reactnative.dev/docs/headless-js-android | DOC | A-cat | verified: the page sits in the Android guides and documents only Android (`HeadlessJsTaskService`) |
| M11-F2 | both debug in their own language with DevTools | R12, F8 | DOC / DER | A-cat | verified (DER over both pages) |
| M11-N1 | Solved: background runs in the window the OS grants; symbol files and `undra symbolicate`; a debugger path into Rust, documented for Xcode, Android Studio and Chrome and tested with LLDB on the iOS simulator. Caveat: iOS background tasks do not run in the simulator, so that handler is tested with fakes. *Was: "Not yet: background execution, step-debugging into Rust, crash-symbol files"* | adr-046 (Accepted, line 3) and its amendment items 8-16; stat:491; sde:prod-ops:65, :106 (LLDB stops at a Rust line on the host and in an iOS simulator process); production.html (#background, #symbols, #debugging); sc:830 (S30) | REPO | fact-check | **corrected**: ADR-046 shipped after the draft. Android Studio and Chrome are documented and checked by `undra doctor` (`android.lldb`, `web.devtools-dwarf`), not run by a test; the cell says so |
| M11-N2 | Not yet: physical-device measurements | res:965-970; res:1034-1038 | REPO | A | verified |

### Class 12: Ecosystem and track record (row 35)

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| M12-K1 | KMP: JetBrains builds it | K11 (JetBrains' own FAQ); ledger-0930 K21 | DOC | A-cat | verified |
| M12-K2 | Google states support | K12 | DOC | A-cat | verified K12: "officially supported by Google for sharing business logic between Android and iOS" |
| M12-U1 | UniFFI: used extensively in Firefox | U1 | RAW | A-cat | verified U1: "used extensively by Mozilla in Firefox mobile and desktop browsers" |
| M12-U2 | its README says it is a long way from 1.0. *Was: "far from 1.0"* | U1 | RAW | A-cat | **corrected** to the README's own words: "a long way from a 1.0 release" |
| M12-F1 | Flutter / React Native: backed by Google, and by Meta and Expo | cat:456 (footnote 35, DER) | DER | A-cat | verified (DER) |
| M12-N1 | Undra: one team, no production users | cat:69-75 | REPO | A | verified |
| M12-N2 | nothing on a registry | README.md:146, :160; roadmap.json (crates.io and Maven Central: Next) | REPO | A | verified |

## 4. What we solved, and how

### The boundary

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| B01 | Five classes carry most of the weight; each gets one measured number | the five sections that follow | DER | A-judge | verified (each section has its number) |
| B02 | A boundary costs twice: once per crossing, and once for what a crossing carries | framing | DER | A-judge | verified (framing) |
| B03 | reads never cross and a write crosses once per transaction | CLAUDE.md:16 (R5) | REPO | A | verified |
| B04 | a write returns one change-set for each store it touched | SPEC §5.5 (:443); sc:225-237 (S09) | REPO | A | verified |
| B05 | ADR-031: the platform applies what has arrived once per frame, merged, with a bounded backlog | adr-031:1 (title; Accepted) | REPO | A | verified |
| B06 | a burst of 1,667 one-row updates to a 10,000-row list is 100,000 a second at 60 Hz | res:947 (heading) | REPO | A | verified |
| B07 | it reaches the iOS app as one drain that costs the main thread 787 µs at the median | res:951 ("787 µs / 1.41 ms", "1,667 → 1 (1 drain)") | REPO | A | verified res:951 |
| B08 | applying each update on its own is estimated at 3.4 to 3.94 ms, 20.4 to 23.7 percent of a frame. *Was: "3.4 to 3.9 ms, 20 to 24 percent"* | res:951 ("3.4 ms to 3.94 ms", "20.4% to 23.7%"); res:955 (the runtime was not reverted: an estimate) | REPO | A | **corrected**: the record's numbers unrounded (23.7 had been rounded up to 24, in the post's favour) |
| B09 | iPhone 17 Pro simulator on an Apple M5 Pro, so not a device | res:874, :951 | REPO | A | verified |
| B10 | ADR-044 turned the C ABI into one function table per core | adr-044:74-75; stat:417 | REPO | A | verified |
| B11 | so two cores can share a process without sharing anything | rev:abi-table (Verdict); stat:417 (`nm` shows one global per core; two cores under ASan) | REPO | A | verified stat:417 |
| B12 | What the table's indirection costs a call is not settled: the implementer measured +0.6 ns on quiet rounds whose load the record does not give. *Was: "the implementer's paired runs put the extra indirection at +0.6 ns on a 49.8 ns synchronous call"* | sde:abi-table:69; rev:abi-table:157-159 | REPO | A | **corrected**: the number stays only with its caveat; the 49.8 ns (res:701) predates the table and is dropped from the sentence |
| B13 | the reviewer, on a loaded machine, measured a median of −5.1 ns inside a spread of ±40 ns, which shows no regression above the noise and cannot confirm a figure that small; a re-measure on a quiet machine is owed. *Was: "could neither confirm nor refute that and asked for a re-measure"* | rev:abi-table:154-166 ("median −5.1 ns", "the spread (±40 ns)", "neither confirms nor refutes"), :223; no re-measure in res or stat:417-512 | REPO | A | **corrected**: the reviewer's own number is now given |
| B14 | through the generated binding a call is about 300 ns on the iOS simulator and 0.47 to 0.68 µs in Chromium. *Was: "3.2 to 3.5 µs in Chromium"* | res:880, :961 (iOS 294 to 302 ns); res:928, :963, :987-989 (web: three runs of 2026-10-02 after ADR-056) | REPO | A | **corrected**: the Chromium figure moved with ADR-056 |
| B15 | against 44 ns for the core alone | res:36 (`dispatch/call_sync/add` 43.9 ns) | REPO | A | verified |
| B17 | ADR-056 brought the Chromium call down from 3.2 to 3.5 µs by taking allocations and lowered private fields off the call path | adr-056 §2 (levers 1-8: allocation, the build target, `#private`), §3 ("3.16 to 3.48 µs" before); res:989 | REPO | fact-check | **added**; verified adr-056 §2-3 |
| B18 | the same changes cut the React Native mirror's work at 100,000 keyed updates a second from 21.5 to 22.0 ms of a 16.7 ms frame to 5.0 to 5.4 ms, on the iPhone simulator under Hermes | adr-056 §4 ("was 21.5 to 22.0 ms ... and is 5.0 to 5.4 ms"); docs/REACT_NATIVE.md:253-258 | REPO | fact-check | **added**; verified adr-056 §4 |
| B16 | The call is still over its targets, which is open work | res:987-994 (iOS 5x its 60 ns target; the web's in-thread row 4 to 6x its 80 ns) | REPO | A | verified |

### State over the boundary

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| S01 | A `Computed<Vec<T>>` was recomputed at every commit and sent whole | res:444-446 | REPO | A | verified |
| S02 | one edited row in a 10,000-row list shipped 352.6 KB and took 176.5 µs | res:456 | REPO | A | verified res:456 |
| S03 | a `DerivedList` is kept from the source's recorded operations on two order-statistic trees | res:446-448; adr-039 | REPO | A | verified |
| S04 | ships the change: 158 bytes for the same edit, in 333 ns measured on the signals crate directly and 392 ns through the runtime, which is the figure on our landing page. *Was: "333 ns and 158 bytes for the same edit"* | res:450-456 (`undra-signals` directly: 333 ns, 158 bytes); res:472-478 (`signals/derived_10k/update_visible`, "through the runtime": 392 ns); `site/data/bench.json` (the landing card: 392 ns, 158 bytes) | REPO | A | **corrected**: the post now gives both harnesses' numbers, so it agrees with the landing page |
| S05 | 375 ns and 158 bytes at 100,000 rows | res:457 | REPO | A | verified |
| S06 | best of three medians. *Was: "core side, best of three medians"* | res:451 | REPO | A | **corrected**: "core side" moved into S04's wording |
| S07 | a contract scenario replays 60,000 seeded operations through the Swift, Kotlin and TypeScript runtimes and checks each view's hash after every change-set | sc:519-528 (S19 step 9) | REPO | A | verified sc:519-528 |
| S08 | a platform still pays its own list copy, once per drain | res:469-470 | REPO | A | verified |
| S09 | for a one-operation patch to a 100,000-row source, 47 µs in TypeScript and 7 µs in Kotlin | res:467 (47.3 µs, 7.2 µs) | REPO | A | verified (rounded down: not in the post's favour) |
| S10 | iOS 15 and 16 mode copies the list on every patch, 48 µs at 10,000 rows | ios-15-16.html ("48 µs for 10,000"); stat:463 | REPO | A | verified |

### The error channel

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| E01 | a failing call throws or rejects with the method's own error, the platform's cancellation, or `UndraCallError` | docs/ERRORS.md:11-19 | REPO | A | verified |
| E02 | a closed set of five cases (cancelled by the core, panicked, refused, unavailable, malformed) | docs/ERRORS.md:21-29 | REPO | A | verified |
| E03 | a command reports through `onError` | docs/ERRORS.md:128-130 | REPO | A | verified |
| E04 | no reply status, cancellation or lost connection reaches `fatalError`, an uncaught exception or an unhandled rejection | docs/ERRORS.md:7-9 | REPO | A | verified |
| E05 | a native panic is caught at the boundary and arrives as `panicked` | docs/ERRORS.md:26; sc:417 (S17) | REPO | A | verified |
| E14 | and the app's `onPanic` receives a structured report with frames its crash reporter can symbolicate (ADR-046) | adr-046 amendment items 1-2; production.html (#crash-reports); stat:491 ("symbolication proven on iOS Release, Android, host and web") | REPO | fact-check | **added**; verified |
| E06 | a computed that panics poisons only itself (ADR-019) | adr-019:77-83 (the amendment: "The typed poisoned state is the signal's, not the store's") | REPO | A | verified |
| E07 | a stream's failure is a typed item on the wire (ADR-036) | adr-036:45 ("Flag 2 carries only the stream's own `E`") | REPO | A | verified |
| E08 | a write from a thread the runtime does not own is refused in every build (ADR-035) | adr-035:46 ("The rule holds in every build") | REPO | A | verified |
| E09 | a wasm core cannot unwind, so it traps | SPEC:646 ("A panic traps the instance (`panic=abort`)") | REPO | A | verified |
| E10 | with `crashRecovery()` the runtime restarts the same module and restores the last snapshot | SPEC:646, :1559; adr-049 | REPO | A | verified |
| E11 | 3.12 to 3.22 ms at the median from the trap to a running core, with a 100 KB state, over five runs in headless Chromium. *Was: "3.1 to 3.2 ms"* | res:682-690 (`ts/recovery_restart_100kb` p50 3.12 ms .. 3.22 ms; a 101,446-byte snapshot; five runs; Chromium 153) | REPO | A | **corrected**: unrounded (3.22 had been rounded down to 3.2, in the post's favour) |
| E12 | recovery is opt-in | SPEC:1559 ("off by default") | REPO | A | verified |
| E13 | a call in flight at the trap fails with `Unavailable` | SPEC:1545 ("a generated call sees UndraCallError.Unavailable") | REPO | A | verified |

### The dev loop

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| D01 | the app on a simulator, an emulator or a page reconnects by itself, with exponential backoff from 250 ms to 5 s. *Was: "on the simulator, the phone or the page"* | adr-051:35-36 (`initialDelay` 250 ms, `maxDelay` 5 s); rev:dev-reload and stat:391 (proven on the iOS simulator, the `undra` AVD and the web) | REPO | A | **corrected**: no physical phone has run it; backoff verified adr-051:35-36 |
| D02 | and observes its stores again | adr-051; docs/DEV_LOOP.md:178 | REPO | A | verified |
| D03 | the core's state is snapshotted in memory before the rebuild and restored into the new core before it listens | cli.html ("snapshotted in this process's memory, never on disk, and restored into the new core"); adr-053 | REPO | A | verified |
| D04 | the dev bar says "Reloaded, state kept" or why not | docs/DEV_LOOP.md:88-94 | REPO | A | verified |
| D05 | the playground's 204 KiB snapshot restores in 1.6 ms inside `undra dev`, which runs a debug build of the core (189 µs in a release build). *Was: "restores in 189 µs" (release core, in process)* | adr-053:355-357 ("inside the dev runner (a *debug* build of the core) `restore` takes 1.6 ms"); sde:dev-reload:83-84 | REPO | A | **corrected**: the dev loop runs the debug core, so 1.6 ms is the number a developer meets; 189 µs kept as the release figure |
| D06 | 74 ms pass from suspending the old core to the new one listening | sde:dev-reload:81; adr-053:357-358 | REPO | A | verified |
| D07 | a page served by `undra dev` shows every store's live value and a timeline of every change-set | docs/DEV_LOOP.md:233-242; adr-054 | REPO | A | verified |
| D08 | a scrubber restores the core to an earlier step while the app follows | docs/DEV_LOOP.md:237-240 | REPO | A | verified |
| D09 | it sits behind a per-run token | docs/DEV_LOOP.md:262-264 | REPO | A | verified |

### Shipping an update, and shipping the app

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| H01 | persisted data records the structure it was written with | SPEC:173 (§2.5); adr-037:1 | REPO | A | verified |
| H02 | a changed type still reads: by name for free, through a `#[undra::migrate]` hook when a name changed | SPEC:378 (§4.5a); updates.html | REPO | A | verified |
| H03 | and never by discarding it silently | adr-037:1 (title) | REPO | A | verified |
| H04 | a queued write the new build cannot carry over becomes a dead letter the app can show | offline.html ("which ones an update could not carry over (dead letters)") | REPO | A | verified |
| H05 | storage ports fail with typed errors (full, locked, corrupt, unavailable, io) on every platform | SPEC:1000, :676; adr-049 | REPO | A | verified |
| H06 | the hello-world web core is 116.7 KB gzipped against a 120 KB budget. *Was: 116.6 KB* | `bench/results/web-size.jsonl:1` (`web/hello-wasm`: 116,690 bytes gzipped, budget 120,000, commit `14aae90`); `bench/budgets.toml:816-819` | REPO | A | **corrected** (refreshed by the measured slot from the record) |
| H07 | the JavaScript a hello-world page loads up front is 15.7 KB gzipped against a 16 KB budget, for the production build of the runtime as an app installs it, with Vite's own preload helper reported beside it and gated on its own (16.2 KB counted in, against 16.6 KB). *Was: 22.1 KB, exactly at its 22,100-byte gate (ts-runtime-16k, ADR-057, moved it)* | `bench/results/web-size.jsonl:2-3` (`web/hello-runtime-js`: 15,680 bytes gzipped, budget 16,000, `bundler_gzipped` 691; `web/hello-runtime-js-with-helper`: 16,191, budget 16,600); `bench/budgets.toml`; adr-057 | REPO | A | **corrected** (refreshed by the measured slot from the record) |
| H08 | CI fails a change that exceeds either | `bench/budgets.toml:793-801` (the gate is min(budget, record × 1.05)); adr-052; stat:383 | REPO | A | verified |
| H10 | the remote transport and the default ports load on first use | `bench/budgets.toml:821-829`; stat:472 | REPO | fact-check | **added**; verified |
| H09 | the browser database adds about 299 KB gzipped, and only to an app that imports it | res:741-743 (299,165 bytes gzipped; "only when it imports `@undra/runtime/db`") | REPO | A | verified |

## 5. What it costs to adopt

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| A01 | The two migration guides describe three levels | from-kmp.html, from-uniffi.html (L1, L2, L3) | REPO | A | verified |
| A02 | L1: a few pure functions move first, the rest of your stack stays | from-kmp.html ("A few pure functions ... The KMP module, Ktor, SQLDelight and your state holders stay") | REPO | A | verified |
| A03 | from UniFFI, plain-data functions in a new crate beside yours | from-uniffi.html ("A new Undra crate beside your UniFFI one. Plain-data functions first") | REPO | A | verified |
| A04 | L2: one screen's state and requests become a store, a query and a mutation | from-kmp.html (L2) | REPO | A | verified |
| A05 | L3: platform services become ports, and persistence and the offline queue move in | from-kmp.html (L3), from-uniffi.html (L3) | REPO | A | verified |
| A06 | the core is Rust, and for an Android-first team Rust is no longer the daily language | from-kmp.html; cat:194 (DER) | REPO / DER | A | verified |
| A07 | a second toolchain: Rust with the iOS, Android and wasm targets, Xcode, and the Android SDK and NDK | CLAUDE.md "Toolchain and local development"; docs/ONBOARDING.md | REPO | A | verified |
| A08 | `undra doctor` checks each of them and prints the command that fixes what is missing. *Was: "runs 34 checks, each with its fix"* | `crates/undra-cli/src/commands/doctor/mod.rs:1-7` | REPO | A | **corrected**: the count moved (37 `Check::new` declarations in `doctor/{rust,ios,android,web,system}.rs` after prod-ops added `android.lldb`, `rust.lldb-formatters` and `web.devtools-dwarf`; one is for contributors only), so the post no longer gives a number |
| A09 | generated Swift defaults to iOS 17 | ios-15-16.html ("iOS 17 by default"); SPEC:25 | REPO | A | verified |
| A10 | an app that supports 15 and 16 takes the `ObservableObject` shape and its extra list copy | ios-15-16.html ("A big list is copied on every patch") | REPO | A | verified |
| A11 | an observed keyed list exists three times: in the core, a baseline copy, and each platform's mirror | SPEC:1314 (the baseline: one clone per observed keyed signal); `site/blog/reads-never-cross-the-boundary` ("What this costs") | REPO | A | verified |
| A12 | `undra adopt` writes a core and the steps but edits none of your project files | adopt.rs:3-7; from-kmp.html | REPO | A | verified |

## 6. What is still open

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| O01 | Objects cannot cross as parameters or return values, and host callbacks cannot be arguments | was: SPEC E0064, E0004 | REPO | A | **removed**: false since ADR-040 and ADR-041 merged (stat:482; SPEC:1079 now says an object crosses as `Arc<T>` or `&T`) |
| O02 | a UniFFI crate meets both on day one | was: from-uniffi.html | REPO | A | **removed**: no longer applies; the guide's own sentence was corrected in this pass |
| O03 | ADR-040 to 042 propose the changes and none has shipped | was: Status Proposed | REPO | A | **removed**: all three Accepted and merged (stat:482, :500) |
| O34 | Generic functions and objects: a generic record or enum crosses as one named type per instantiation; generic objects, stores, functions and methods do not cross | types.html:96-118; SPEC:1051 (E0002) | REPO | fact-check | **added**; verified types.html:118 |
| O04 | the pagination recipe builds an infinite list on a keyed list | pagination.html | REPO | A | **removed** with its bullet: true, but no longer an open item (pagination.html:186 now points at `infinite` queries and `Lazy<T>`) |
| O05 | there is no paged query type and no interval refetch | was: pagination.html; q:33-36 | REPO | A | **removed**: false since ADR-043 (stat:500; q:37-56) |
| O06 | ADR-043 proposes both | was: Status Proposed | REPO | A | **removed**: Accepted and merged |
| O07 | OS background execution, step-debugging into Rust and crash-symbol files are not built; ADR-046 proposes them | was: adr-046 Proposed | REPO | A | **removed**: built and merged (stat:491); the matrix row says what shipped (M11-N1) |
| O08 | the piece is in flight | was: stat checkpoint 21 | REPO | A | **removed**: merged (stat:491) |
| O09 | Flutter and Dart: not started; the plan was to decide after React Native, which has shipped | design:97-98, :170; stat:343 (G1 landed); no Dart piece in stat:300-512 | REPO / DER | A | verified |
| O10 | the Kotlin runtime is plain JVM, but no desktop target ships | cat:152 (E30); roadmap.json ("Desktop targets": Later) | REPO | A | verified |
| O11 | the CLI does not support Windows | README.md:147 ("Not supported yet: Windows") | REPO | A | verified |
| O12 | the TypeScript runtime needs Node 20 or later | `runtimes/ts/@undra/runtime/package.json:67-69` | REPO | A | verified |
| O13 | there is no guide to sharing a core with a server | `site/docs/cookbook/index.html` and `site/docs/*.html` have no server guide; cat:483 (M-13) | REPO | A | verified |
| O14 | nothing is on crates.io, Maven Central, npm or Homebrew yet; the install channels go live with the first tagged release | README.md:146, :160; roadmap.json (Distribution: Now; crates.io and Maven Central: Next) | REPO | A | verified |
| O15 | a queued write's optimistic placeholder and its `invalidates` targets do not survive a restart | q:87-89 | REPO | A | verified |
| O16 | the write replays, and the screen refetches only the mutation's own key | q:88-89 | REPO | A | verified |
| O17 | the web database serves one tab per origin | db.html; stat:453 | REPO | A | verified |
| O18 | a migration that contains its own `COMMIT` is open | stat:453 ("Open: a migration containing its own `COMMIT`"); no later checkpoint closes it | REPO | A | verified |
| O35 | `undra dev` carries the stores across a reload but not query handles, which need a decision record of their own | stat:391; docs/DEV_LOOP.md:254 | REPO | fact-check | **added**; verified |
| O19 | the binding call path is over its targets: 0.32 to 0.44 µs for the runtime's synchronous call in Chromium against 80 ns. *Was: "3.2 to 3.9 µs through the generated TypeScript against 80 ns"* | res:929, :988 (the 80 ns target is the in-thread `callSync` row's; the generated call's row has none, res:928) | REPO | A | **corrected**: the numbers moved with ADR-056, and the target belongs to the synchronous row |
| O20 | about 300 ns on the iOS simulator against 60 ns | res:880, :961 (294 to 302 ns; target ≤ 60 ns) | REPO | A | verified |
| O21 | on React Native, 100,000 updates a second do not fit a frame | was: docs/REACT_NATIVE.md | REPO | A | **removed**: false since ADR-056 (docs/REACT_NATIVE.md:253-265: about 5 ms of a 16.7 ms frame on the simulator); the new figure is B18 |
| O22 | and the docs advise staying near 10,000 a second or below | was: docs/REACT_NATIVE.md | REPO | A | **removed**: the docs no longer say so (docs/REACT_NATIVE.md:264-265) |
| O36 | iOS background tasks do not run in the simulator, so the iOS background handler is tested with fakes and has not run under the OS scheduler here | adr-046 amendment item 8; production.html ("BackgroundTasks does not run in the simulator") | REPO | fact-check | **added**; verified |
| O23 | every number is from a host, a simulator, an emulator or Chromium | res:874, :898, :922, :965-970 | REPO | A | verified |
| O24 | the iOS 15 and 16 mode is proven by compilation and a runtime probe on iOS 26.5, not on an older runtime | stat:463 | REPO | A | verified |
| O25 | KMP has JetBrains and Google's stated support; UniFFI ships in Firefox | K12; U1 | DOC / RAW | A-cat | verified (as M12-K2, M12-U1) |
| O26 | Undra is one team's work with no production users | cat:69-75 | REPO | A | verified |
| O27 | on Android, shared Kotlin is plain Kotlin with no boundary; Undra crosses JNI | cat:191 (KMP-B1); from-kmp.html ("Android has a boundary") | REPO | A | verified |
| O28 | for an Android-first team Kotlin is the daily language, and in our judgement Rust is the adoption wall | cat:194 (KMP-B4, DER) | DER | A-judge | verified (worded as judgement) |
| O29 | UniFFI serves Python and Ruby; Undra serves three UIs, and React Native on top | U1; docs/REACT_NATIVE.md | RAW | A-cat | verified |
| O30 | Expo's EAS Update ships JavaScript without a new binary | R14 docs.expo.dev/eas-update/introduction/ | DOC | A-cat | verified R14: an app can "update its own non-native pieces (such as JS, styling, and images) over-the-air" |
| O31 | a native Undra core ships with the app; a web core updates with the site | cat:488 (M-18); SPEC §7 (the web core is a wasm module the page loads) | DER | A-judge | verified (architecture) |
| O32 | Undra does not share UI, by decision; for one codebase on every screen, look at Flutter, React Native or Compose Multiplatform | README.md:191-192; cat:195-196, :326, :297 | REPO | A | verified |
| O33 | writing it three times needs no new language or toolchain and keeps the best native tooling, with no boundary to debug across | cat:365-368 | DER | A-judge | verified (judgement, from cat:365-368) |

## 7. The default

| ID | Claim as written | Source | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|
| C01 | Default is a claim about which question comes first, not about every team | editorial | DER | A-judge | verified (editorial) |
| C02 | the open rows above are few enough to check against your own requirements in an afternoon | today's tally: 2 open rows, 6 partial (section 11) | DER | A-judge | verified (judgement; the tally it rests on is re-derived) |
| C03 | start with L1: move one function, call it from Swift, Kotlin and TypeScript | from-kmp.html ("Move one function, call it from Kotlin and Swift, ship") | REPO | A | verified (advice; the guide names Kotlin and Swift, the post adds TypeScript) |

## 8. Links the post makes (not factual claims)

ADR files linked on GitHub (`.10x/adrs/`): ADR-019, 031, 032, 035, 036, 037, 039, 040, 042, 043, 044, 046, 049, 051, 052, 053, 054, 055, 056
(each exists in this tree; the GitHub `main` URLs resolve once this branch merges). Review: `2026-10-02-abi-table-review.md`. Record:
`.10x/decisions/sde/dev-reload.md`. Docs pages added by the fact-check: `objects.html`, `callbacks.html`, `types.html` (and `#generic-data-types`),
`paging.html`, `production.html`. Benchmark anchors in `bench/RESULTS.md`, each checked against its heading: `#the-adr-031-drain-1667-one-update-keyed-patches-on-10000-rows-per-frame-100000-a-second-at-60-hz`
(res:947), `#5-a-computed-list-over-a-keyed-list-cost-the-list-again-a-derived-list-costs-the-change-adr-039` (res:442),
`#web-recovery-typescript-adr-049` (res:674), `#device-numbers-ios-android-web` (res:789). `site/scripts/check-links.mjs` passes for every
site-relative link.

## 9. Left out because it could not be sourced, or on purpose

* From the blueprint's claims that cat:510-516 found unbacked on 1 October: "time-travel any transaction in a devtools inspector" (now backed and
  claimed in a narrower form: a devtools page with a change-set timeline and restore), "newtypes stay typed" and "lazy collections" (now backed by
  ADR-042 and ADR-043 and claimed), "the optimistic state survives app restarts" (still false, q:87-89; the post says the opposite), "the core
  runs in a Web Worker by default", a Telemetry or Push port, "`undra adopt` adds the package and a bootstrap", command priorities (not claimed).
* The +0.6 ns cost of ADR-044's table: kept only with the reviewer's −5.1 ns inside ±40 ns (B12, B13); no quiet re-measure exists.
* Android Studio and Chrome step-debugging into Rust: documented and checked by `undra doctor`, not run by a test; the matrix cell says "documented".
* The `+37` TypeScript and `+32` Kotlin suites of stat:511: not in the post's test counts (MI08 is the main suites only).
* Any claim about UniFFI on rows the catalogue marks "not assessed" (background execution, debugging, symbolication, device measurement).
* Crux in the matrix, and Capacitor and "write it three times" beyond the premise's count and one bullet (cat:365-368).
* Any speed comparison with another tool: none is made.

## 10. Stale statements elsewhere on the site, fixed in the fact-check pass

* `site/blog/reads-never-cross-the-boundary/index.html`: "Derived lists cross whole" is now "A computed list crosses whole", naming `DerivedList`
  (ADR-039, 158 bytes against 352.6 KB, res:456); the playground snippet's `visible` is a `DerivedList<Todo>` as in
  `examples/playground/core/src/todos.rs:65-66`, and the Kotlin and TypeScript snippets' doc comments match the generated files
  (`Stores.kt:2632`, `stores.ts:2457`). `dateModified` 2026-10-02. Its error section already matched ADR-032 amendment A.
* `site/blog/undra-vs-kotlin-multiplatform/index.html`: "The Android runtime has no remote transport yet" is replaced by what ADR-051 and ADR-053
  ship, with a dated correction; "Undra v1 has a key-value port and a query layer, not SQLite" now names the opt-in `Db` port (ADR-048).
  `dateModified` 2026-10-02.
* `site/docs/cookbook/from-uniffi.html` and `from-kmp.html` (flagged at stat:444, "the two migration pages need the H4 fact-check"): "What does
  not map yet" listed objects, callbacks, generics and newtypes; now generic functions and objects and other hosts, with a row for callback
  interfaces in the mapping table. `from-kmp.html`'s "Objects as parameters and return values do not cross" is now the generics limit.
* `site/data/roadmap.json`: Track A (ADR-034), the Android adapters and React Native (all merged, stat:343-359) moved from Now to Since v1.0, as did
  the migration guides (stat:444) from Next; "A first-party inspector" left Later (devtools shipped, ADR-054); the stale "102.7 KB" is gone;
  ADR-044, ADR-045 and ADR-056 added to Since v1.0; `updated` 2026-10-02.
* `site/data/pending.json`: `docs/realtime.html` is live; the entry is removed.

## 11. The 36-row tally behind MI02 and MI04

"1 Oct" is the catalogue's "Undra today" column (cat:382-417; row 2 "yes native, part. web" counted as yes). "Today" is the fact-check's reading
of `main` `da4fbbe`. Counts: 1 Oct = 10 yes, 9 part., 16 no, 1 n/a. **Today = 27 solved, 6 partial, 2 open, 1 n/a** (the draft's 22 / 7 / 6 / 1
was right for `d1b35b5`; six rows moved with the pieces merged after it).

| ID | Row (cat) | 1 Oct | Today | Evidence for "today" | Basis | Checked by | Fact-check |
|---|---|---|---|---|---|---|---|
| T01 | 1 Typed domain errors across the boundary | yes | solved | docs/ERRORS.md:11-29; adr-032 | REPO | A | verified |
| T02 | 2 A bug in shared logic does not end the app | yes | solved | adr-019:77-83; docs/ERRORS.md:26; sc:417 (S17); adr-049, sc:578 (S22); res:690; adr-046 (the report) | REPO | A | verified |
| T03 | 3 UI reads shared state without crossing | yes | solved | CLAUDE.md:16; sc:267-268 (S11) | REPO | A | verified |
| T04 | 4 Updates cost O(change), including large lists | part. | solved | adr-027, adr-039; res:340-376, :442-478; sc:239 (S10), :499 (S19). Caveat: a plain `Computed<Vec<T>>` still crosses whole; the derived list is the O(change) path | REPO | A | verified |
| T05 | 5 Shared state observable natively | yes | solved | cat:134 (E12); api-swift.html, api-kotlin.html, api-typescript.html | REPO | A | verified |
| T06 | 6 Cancellation from the UI, streams with backpressure | yes | solved | sc:168 (S06), :185 (S07); adr-036 | REPO | A | verified |
| T07 | 7 Server-state cache with optimistic updates and rollback | yes | solved | q:27-33, :69-80; sc:270 (S12), :290 (S13); queries.html | REPO | A | verified |
| T08 | 8 Offline mutation queue that survives a restart intact | part. | partial | sc:308 (S14); q:87-89 (optimistic state and `invalidates` do not survive) | REPO | A | verified |
| T09 | 9 Persisted state with migrations | no | solved | adr-037; rev:persistence | REPO | A | verified |
| T10 | 10 Paged and infinite lists, and polling | no | **solved** (was partial) | adr-043 (Accepted); q:37-56; sc:889 (S32), :918 (S33); paging.html, polling.html; stat:500 | REPO | fact-check | **corrected** |
| T11 | 11 Structured local database (SQL) | no | solved | adr-048; db.html; sc:646 (S25). Caveat: one tab per origin on the web | REPO | A | verified |
| T12 | 12 WebSocket and real-time streams | no | solved | adr-047; realtime.html; sc:602 (S23) | REPO | A | verified |
| T13 | 13 OS background execution | no | **solved** (was open). Caveat: the iOS handler cannot run under BGTaskScheduler in the simulator | adr-046 §3 and amendment items 6-10; sc:830 (S30); production.html#background; stat:491 | REPO | fact-check | **corrected** |
| T14 | 14 Default HTTP, storage and connectivity adapters on iOS, Android and web | part. | solved | rev:android-adapters; stat:351, :399; ports.html | REPO | A | verified |
| T15 | 15 Live reload of shared logic on a device, keeping state | part. | solved | adr-051, adr-053; rev:dev-reload; stat:391 (iOS simulator, `undra` AVD, web; no physical phone, D01) | REPO | A | verified (scope noted) |
| T16 | 16 State inspector, transaction timeline, time travel | no | solved | adr-054; docs/DEV_LOOP.md:225-260; rev:devtools | REPO | A | verified |
| T17 | 17 Step-debug from UI code into shared code | no | **solved** (was open). Scope: tested with LLDB on the host and the iOS simulator; Android Studio and Chrome documented | adr-046 §2 and amendment items 12, 16; sde:prod-ops:65, :106; production.html#debugging | REPO | fact-check | **corrected** |
| T18 | 18 Previews and UI tests without the real core | no | solved | adr-055; docs/TESTING.md; rev:testkit | REPO | A | verified |
| T19 | 19 Deterministic tests (virtual time, fake I/O) | yes | solved | cat:141 (E19); adr-055 | REPO | A | verified |
| T20 | 20 Compatibility check between bindings and library | yes | solved | SPEC:151; sc:396 (S16) | REPO | A | verified |
| T21 | 21 Generated Swift passes native review | yes | solved (judgement) | CLAUDE.md:14 (R3); `crates/undra-bindgen/tests/golden/`; adr-032, adr-045 | DER | A-judge | verified (judgement) |
| T22 | 22 Generics across the boundary | no | **partial** (was open) | adr-042 (Accepted); types.html:96-118 (named instantiations; generic objects, functions and methods stay out); sc:861 (S31) | REPO | fact-check | **corrected** |
| T23 | 23 Objects, callbacks and listeners as arguments and returns | part. | **solved** (was partial) | adr-040, adr-041 (Accepted); sc:705 (S27), :748 (S28); objects.html, callbacks.html; stat:482, :509 | REPO | fact-check | **corrected** |
| T24 | 24 Supports iOS 15 and 16 | no | solved (caveat) | adr-045; rev:ios-floor; stat:463. Caveat: no iOS 15/16 runtime has run it | REPO | A | verified |
| T25 | 25 First-party web target with a DOM UI | yes | solved | cat:131 (E9); web.html | REPO | A | verified |
| T26 | 26 Desktop | part. | partial | cat:152 (E30); roadmap.json (Later) | REPO | A | verified |
| T27 | 27 Reuse of the shared code on a server | part. | partial | package.json:67-69 (`node >=20`); cat:448 (footnote 27, DER) | REPO / DER | A | verified |
| T28 | 28 React Native UI on top | no | solved | adr-038; docs/REACT_NATIVE.md; rev:react-native; rev:rn-adapters | REPO | A | verified |
| T29 | 29 Flutter UI on top | no | open | design:97-98, :170; no Dart piece in stat:300-512 | REPO | A | verified |
| T30 | 30 Two independent libraries built with it in one app | no | solved | adr-044; rev:abi-table; several-cores.html; sc:671 (S26). Caveat: React Native runs one instance per namespace per process (docs/REACT_NATIVE.md:271-280) | REPO | A | verified |
| T31 | 31 Incremental adoption in an existing app | part. | partial | from-kmp.html, from-uniffi.html; adopt.rs:3-7 (edits are manual) | REPO | A | verified |
| T32 | 32 Build-system integration | no | solved | rev:tooling; stat:375 | REPO | A | verified |
| T33 | 33 Crash symbolication of shared code | no | **solved** (was open) | adr-046 §1 and amendment items 12-14; stat:491 ("symbolication proven on iOS Release, Android, host and web"); production.html#symbols; `crates/undra-cli/src/commands/symbolicate.rs` | REPO | fact-check | **corrected** |
| T34 | 34 Size, startup and list performance measured on devices | part. | partial | res:874-970 (simulator, emulator, Chromium rows; no physical device) | REPO | A | verified |
| T35 | 35 Ecosystem, backing, production record | no | open | cat:69-75 | REPO | A | verified |
| T36 | 36 Shared UI if the product wants it | n/a | n/a (by decision) | README.md:191-192 | REPO | A | verified |
