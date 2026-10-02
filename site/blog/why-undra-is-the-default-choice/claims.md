# Claims ledger: "Why Undra is the default choice"

Post: `site/blog/why-undra-is-the-default-choice/index.html`, dated 2026-10-02. Piece: `default-choice-post` (H4).
Written by the author of the post on 2026-10-02 from this worktree (`wt/default-choice-post`, based on `main` `d1b35b5`).
Same purpose as `.10x/reviews/2026-09-30-blog-claims.md`: let a fact-checker verify every factual sentence quickly and independently.
**Every row ends "fact-check: pending": the integrator's adversarial pass is not done.**

## How to read the columns

**Aliases for paths.** `cat` = `.10x/specs/2026-10-01-competitive-limitations.md` (`cat:173` is a line of it; a catalogue source id such as K2 or U5 is
its section 12, which has the URL and the page date). `res` = `bench/RESULTS.md`. `adr-NNN` = `.10x/adrs/ADR-NNN-*.md`. `rev:x` =
`.10x/reviews/*-x-review.md`. `stat` = `.10x/status.md`. `sde:x` = `.10x/decisions/sde/x.md`. `design` = `.10x/specs/2026-10-01-v1x-default-choice-design.md`.
`ledger-0930` = `.10x/reviews/2026-09-30-blog-claims.md`. Everything else is a repository path.

**Basis**, the same codes as the catalogue (section 1):

| Code | Meaning |
|---|---|
| RAW | a raw file (README, source template) the catalogue read verbatim on 2026-10-01 |
| DOC | an official documentation page the catalogue read with a fetch tool that returns a model-written extract; the sentence should be confirmed on the page |
| 3P | a third-party article |
| REPO | this repository: a file, a line, a review, a benchmark record, a contract scenario |
| DER | **judgement or inference** from the rows it names, or a reading of a page's silence. The post words these as "as we read" or "its documentation describes no" |

**Checked by.** `A` = the author opened the cited repository path (or read the cited section of the catalogue) in this worktree on 2026-10-02 and it says
what the claim says. `A-cat` = the claim about another tool is carried from the catalogue row without re-fetching the competitor's page (the catalogue fetched
them on 2026-10-01; the fact-check should re-fetch). `A-judge` = the author's judgement from the rows named. Competitor pages move; nothing here was re-fetched.

## 1. Lede and "How to read this page"

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| P01 | A team choosing how to share logic across iOS, Android and the web asks "what will this stop me doing?" first | design:6-14 (the bar: "no engineer declines Undra because of something it cannot do") | DER | A-judge; fact-check: pending |
| P02 | We catalogued 68 limitations of KMP, UniFFI, Crux, React Native, Flutter, Capacitor and writing the logic three times | cat:51-60 (18 + 15 + 9 + 9 + 9 + 3 + 5 = 68) | REPO | A; fact-check: pending |
| P03 | each dated and sourced where a source exists | cat:85-91 (dates and primary sources); cat:353-355 (the "write three times" rows are DER with no primary source); cat:214, 222 (UniFFI rows read a page's silence) | REPO | A; fact-check: pending |
| P04 | and mapped every one to what Undra's repository does about it | cat:48-66, 105-115 (SOLVED / PLANNED / MISSING / N/A, as of 1 Oct) | REPO | A; fact-check: pending |
| P05 | Statements about other tools are as of 1 October 2026 | cat:85-88 | REPO | A; fact-check: pending |
| P06 | Timings are measured on an Apple M5 Pro host, its iPhone simulator, its Android emulator or headless Chromium | res:16-24 (machine); res:852-858 (iPhone 17 Pro simulator, Android emulator, Chromium 153) | REPO | A; fact-check: pending |
| P07 | none is from a physical device | res:870-878 (Pending hardware), res:940-945 (Still waiting for) | REPO | A; fact-check: pending |
| P08 | the claims ledger and the catalogue are linked | the two GitHub links in the callout resolve to this file and to `cat` once merged | REPO | A; fact-check: pending |

## 2. The matrix: introduction

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| MI01 | The catalogue's matrix lists 36 things a team needs | cat:372-417 | REPO | A; fact-check: pending |
| MI02 | On 1 October Undra was marked as having 10 of them, part of 9 and none of 16, with one not applicable | the "Undra today" column of cat:382-417, tallied in section 11 below (T01 to T36, column "1 Oct") | REPO | A; fact-check: pending |
| MI03 | Since then the pieces below have merged, each after an adversarial review | stat checkpoints 5-21 (each row names its review); `.10x/reviews/2026-10-01-*` and `2026-10-02-*` | REPO | A; fact-check: pending |
| MI04 | Counting again today: 22 solved, 7 partial, 6 open and 1 left out by decision | tally in section 11 below (column "today"); **the author's own count, the number most worth re-deriving** | DER | A-judge; fact-check: pending |
| MI05 | The table groups the 36 rows into 12 classes | the row labels of the post's table ("rows 3 to 6", ...) cover rows 1-36 once each: 3-6, 1-2, 21-23, 7-10, 11/12/14, 15-16, 18-19, 20/24/30, 25-29 + 36, 31-32, 13/17/33/34, 35 | REPO | A; fact-check: pending |
| MI06 | It leaves out Crux, which its own post covers | `site/blog/undra-vs-uniffi-and-crux/index.html`; cat:243-270 | REPO | A; fact-check: pending |

## 3. The matrix: one block per class

Each cell is one row of the post's table. `K` = Kotlin Multiplatform, `U` = UniFFI, `F` = Flutter / React Native, `N` = Undra.

### Class 1: State reaches the UI (rows 3 to 6)

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| M01-K1 | On iOS Swift copies a Kotlin collection whole | cat:173 (KMP-5); DOC K2 kotlinlang.org/docs/native-objc-interop.html | DOC | A-cat; fact-check: pending |
| M01-K2 | observing a `Flow` takes SKIE or similar | cat:183 (KMP-15), cat:169 (KMP-2); DOC K1, K16 skie.touchlab.co | DOC | A-cat; fact-check: pending |
| M01-U1 | UniFFI's documentation describes no observable state | cat:214 (UNI-1): absence from RAW U1, U2 | DER | A-cat; fact-check: pending |
| M01-U2 | reading Rust state is a call across the boundary | cat:215 (UNI-2): RAW U7 (lowered to a byte buffer) plus the inference | DER | A-cat; fact-check: pending |
| M01-F1 | Flutter / React Native: no language boundary | cat:422 (footnote 1: "logic and UI share a language"), cat:384 (row 3: yes, yes) | DER | A-cat; fact-check: pending |
| M01-F2 | React Native runs logic on the JS thread; a frame it misses is dropped | cat:283 (RN-1); DOC R3 reactnative.dev/docs/performance | DOC | A-cat; fact-check: pending |
| M01-N1 | Undra: reads are local | CLAUDE.md:16 (R5); contract-tests/scenarios.md:254 (S11: 1,000 reads, the crossing counter does not move) | REPO | A; fact-check: pending |
| M01-N2 | a write crosses once | CLAUDE.md:16; contract-tests/scenarios.md:223 (S09: one transaction, one change-set) | REPO | A; fact-check: pending |
| M01-N3 | lists cross as O(change) | adr-027; adr-039; res:340-376 (finding 1), res:442-480 (finding 5); contract-tests/scenarios.md:237 (S10), :497 (S19) | REPO | A; fact-check: pending |
| M01-N4 | delivery is merged per frame | adr-031; contract-tests/scenarios.md:476 (S18); res:852-858 | REPO | A; fact-check: pending |

### Class 2: Errors and crashes (rows 1, 2)

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| M02-K1 | Only `@Throws` exceptions become Swift errors; others end the program | cat:172 (KMP-4); DOC K2 | DOC | A-cat; fact-check: pending |
| M02-U1 | As we read its Swift template, a Rust panic in a non-`Result` function stops the process | cat:217 (UNI-4): RAW U5 (`try!` in macros.swift, `rustPanic` in Helpers.swift) plus the consequence | RAW / DER | A-cat; fact-check: pending |
| M02-F1 | Flutter swaps a failed widget for a gray background in release | cat:329 (FL-B4); DOC F10 docs.flutter.dev/testing/errors | DOC | A-cat; fact-check: pending |
| M02-F2 | Native modules send codes and messages | cat:422 (footnote 1: R5; F2 `result.error("UNAVAILABLE", ...)`); DOC F2 | DOC | A-cat; fact-check: pending |
| M02-N1 | Undra: one closed `UndraCallError` set on every platform | docs/ERRORS.md:21 ("a closed set of five cases"), :11-19 (the same on Swift, Kotlin, TypeScript) | REPO | A; fact-check: pending |
| M02-N2 | panics are caught at the boundary | docs/ERRORS.md:21-30 (`panicked`: "The core caught the panic and keeps working"); contract-tests/scenarios.md:415 (S17) | REPO | A; fact-check: pending |
| M02-N3 | a trapped wasm core restarts from its snapshot | adr-049; docs/SPEC.md:603; contract-tests/scenarios.md:576 (S22); site/docs/web.html | REPO | A; fact-check: pending |

### Class 3: What can cross (rows 21 to 23)

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| M03-K1 | Swift sees an Objective-C header, or Swift export (Alpha), which erases generics | cat:168 (KMP-1); DOC K1, K2 | DOC | A-cat; fact-check: pending |
| M03-U1 | UniFFI: objects and foreign traits cross | cat:234 (UNI-B1); DOC U8 mozilla.github.io/uniffi-rs/latest/types/interfaces.html | DOC | A-cat; fact-check: pending |
| M03-U2 | its documented types list no generics | cat:443 (footnote 22): RAW U13, absence | DER | A-cat; fact-check: pending |
| M03-F1 | `flutter_rust_bridge` takes arbitrary types and closures | cat:331 (FL-B6); RAW F12 | RAW | A-cat; fact-check: pending |
| M03-F2 | React Native native modules need a spec and code per platform | cat:284 (RN-2); DOC R5 | DOC | A-cat; fact-check: pending |
| M03-N1 | Undra: records, enums and errors become native sum types | cat:125 (E3); docs/SPEC.md section 10.1-10.3; crates/undra-bindgen/tests/golden/; docs/ERRORS.md:11-19 | REPO | A; fact-check: pending |
| M03-N2 | no Objective-C | cat:123 (E1); docs/SPEC.md section 10; the generated Swift is Swift (examples/playground/generated/swift) | REPO | A; fact-check: pending |
| M03-N3 | Not yet: objects as parameters or returns, callbacks, generics, newtypes | docs/SPEC.md:951 (E0064), :925 (E0004), :923 (E0002), :928 (E0007: tuple and unit structs); adr-040, adr-041, adr-042 (Status: Proposed, line 3); stat checkpoint 21 ("In flight: objects-callbacks"); worktree `.work/objects-callbacks` exists, unmerged | REPO | A; fact-check: pending |

### Class 4: Data layer (rows 7 to 10)

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| M04-K1 | KMP: libraries Room, SQLDelight, Store | cat:182 (KMP-14); DOC K12; RAW K18, K19 | DOC / RAW | A-cat; fact-check: pending |
| M04-K2 | SQLDelight has compile-time migration checks | cat:192 (KMP-B2); RAW K19 ("verifies your schema, statements, and migrations at compile-time") | RAW | A-cat; fact-check: pending |
| M04-U1 | UniFFI's documentation describes no cache, mutation, offline or persistence layer | cat:222 (UNI-9): absence from U1, U2 | DER | A-cat; fact-check: pending |
| M04-F1 | Flutter / React Native: libraries, TanStack Query | cat:291 (RN-8); DOC R18 | DOC | A-cat; fact-check: pending |
| M04-F2 | drift (with migrations) | cat:330 (FL-B5), cat:430 (footnote 9: "drift's schema migrations"); DOC F13 | DOC | A-cat; fact-check: pending |
| M04-N1 | Undra: cache, dedup, retry, optimistic rollback | docs/SPEC.md section 9; crates/undra-query/src/lib.rs:20-40; contract-tests/scenarios.md:268 (S12), :288 (S13); site/docs/queries.html | REPO | A; fact-check: pending |
| M04-N2 | an offline queue that survives a restart | contract-tests/scenarios.md:306 (S14); crates/undra-query/src/lib.rs:50-66; site/docs/cookbook/offline.html (the kill-and-relaunch recipe) | REPO | A; fact-check: pending |
| M04-N3 | migrating persisted state | adr-037; docs/SPEC.md:162, :195, :350; contract-tests/scenarios.md:51 (S14 steps 7-9, S15 steps 11-14: two builds); rev:persistence (Verdict, :18) | REPO | A; fact-check: pending |
| M04-N4 | Not yet: optimistic placeholders after a restart | crates/undra-query/src/lib.rs:62-66 ("the `invalidates` targets of a queued mutation are remembered in memory only: after a restart ... there is no optimistic update left to roll back") | REPO | A; fact-check: pending |
| M04-N5 | Not yet: paged queries | adr-043 (Status: Proposed, line 3); site/docs/cookbook/pagination.html ("There is no paged query type yet (`Lazy<T>` is rejected in v1)") | REPO | A; fact-check: pending |
| M04-N6 | Not yet: polling | crates/undra-query/src/lib.rs:33-36 ("timed refetching is not in the v1 contract") | REPO | A; fact-check: pending |

### Class 5: Storage, sockets, services (rows 11, 12, 14)

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| M05-K1 | KMP: Ktor, DataStore, Room, SQLDelight; Google's Jetpack libraries are KMP-ready | cat:435 (footnote 14), cat:182, cat:192; DOC K12; RAW K19 | DOC | A-cat; fact-check: pending |
| M05-U1 | UniFFI's documentation describes no adapters; SQL and sockets are Rust crates you bring | cat:432-433 (footnotes 11, 12: "any Rust SQLite crate, native only (DER)"; sockets "lib"); cat:395 (row 14, U: no) | DER | A-cat; fact-check: pending |
| M05-F1 | React Native has WebSocket and `expo-sqlite` | cat:433 (R13), cat:432 (R15); DOC | DOC | A-cat; fact-check: pending |
| M05-F2 | Flutter has `drift` and `web_socket_channel` | cat:432 (F13), cat:433 (F14); DOC | DOC | A-cat; fact-check: pending |
| M05-N1 | Undra: default adapters for HTTP, storage, files and connectivity on iOS, Android, web and React Native | cat:144 (E22, now outdated for Android); rev:android-adapters (Verdict, :12); stat checkpoint 8 (Android: Kv, SecureStore, Fs, Http, Connectivity, Lifecycle), checkpoint 14 (React Native: all ten ports); site/docs/ports.html; docs/SPEC.md:871 | REPO | A; fact-check: pending |
| M05-N2 | opt-in WebSocket, SSE and SQL ports | adr-047, adr-048 (Accepted); rev:ports (Verdict, :16); site/docs/realtime.html; site/docs/db.html; contract-tests/scenarios.md:600-644 (S23-S25) | REPO | A; fact-check: pending |
| M05-N3 | Not yet: the web database serves one tab per origin | site/docs/db.html ("one tab holds at a time: a second tab of the same app gets `Unavailable`"); stat checkpoint 20 ("Open: ... the web Db serves one tab per origin") | REPO | A; fact-check: pending |

### Class 6: Dev loop and tooling (rows 15, 16)

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| M06-K1 | Compose Hot Reload is desktop JVM only | cat:179 (KMP-11); DOC K6 | DOC | A-cat; fact-check: pending |
| M06-K2 | Kotlin/Native build speed is a standing priority | cat:178 (KMP-10); DOC K5, K8 (JetBrains' roadmap names build speed first among iOS priorities) | DOC | A-cat; fact-check: pending |
| M06-U1 | UniFFI: each change rebuilds the library and the app; its documentation describes no dev server or devtools | cat:226-227 (UNI-13, UNI-14) | DER | A-cat; fact-check: pending |
| M06-F1 | Both ship DevTools | cat:299 (RN-B3), cat:327 (FL-B2); DOC R12, F8 | DOC | A-cat; fact-check: pending |
| M06-F2 | Flutter hot reload skips native code | cat:315 (FL-4); DOC F3 | DOC | A-cat; fact-check: pending |
| M06-F3 | React Native Fast Refresh can fall back to a full reload | cat:286 (RN-4); DOC R8 | DOC | A-cat; fact-check: pending |
| M06-N1 | Undra: `undra dev` reconnects | adr-051; site/docs/cli.html:113-118; rev:dev-loop (Verdict, :10) | REPO | A; fact-check: pending |
| M06-N2 | keeps state across a rebuild | adr-053; docs/DEV_LOOP.md:88; rev:dev-reload (Verdict, :14) | REPO | A; fact-check: pending |
| M06-N3 | devtools with time travel | adr-054; docs/DEV_LOOP.md:225-260; rev:devtools (Verdict, :12) | REPO | A; fact-check: pending |
| M06-N4 | Gradle, Xcode and Vite run `undra build` | rev:tooling (Verdict, :18; :21 "Gradle's `undraBuild` runs before `preBuild`"); stat checkpoint 11 | REPO | A; fact-check: pending |
| M06-N5 | Not yet: query handles across a reload | stat checkpoint 13 ("Open: query handles do not survive a reload (needs its own ADR)") | REPO | A; fact-check: pending |

### Class 7: Testing and previews (rows 18, 19)

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| M07-K1 | `kotlinx-coroutines-test` gives virtual time in multiplatform tests | cat:440 (footnote 19: "virtual time, multiplatform"); DOC K21 | DOC | A-cat; fact-check: pending |
| M07-U1 | UniFFI's documentation describes no fakes for platform I/O; you build the doubles | cat:225 (UNI-12) | DER | A-cat; fact-check: pending |
| M07-F1 | Flutter's widget previewer supports no `dart:ffi` or native plugins | cat:319 (FL-8); DOC F9 | DOC | A-cat; fact-check: pending |
| M07-N1 | Undra: `PreviewCore` runs your real core with fakes and a manual clock on Swift, Kotlin and TypeScript | docs/TESTING.md:5-17 (Rust's counterpart is `Harness`, which the post does not claim); adr-055; rev:testkit (Verdict, :11) | REPO | A; fact-check: pending |
| M07-N2 | a recorded session replays in tests | docs/TESTING.md:8-9 (record and replay), docs/SPEC.md:1543 (the `undra.recording` format) | REPO | A; fact-check: pending |

### Class 8: Compatibility and floors (rows 20, 24, 30)

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| M08-K1 | KMP: iOS 15.0 default minimum | cat:197 (KMP-B7); DOC K15 | DOC | A-cat; fact-check: pending |
| M08-K2 | several KMP frameworks in one app duplicate dependencies | cat:177 (KMP-9); DOC K7 | DOC | A-cat; fact-check: pending |
| M08-U1 | As we read its Swift template, a library and bindings that disagree stop the process | cat:218 (UNI-5): RAW wrapper.swift (`fatalError`, "UniFFI API checksum mismatch"); ledger-0930 U08 | RAW | A-cat; fact-check: pending |
| M08-F1 | React Native's minimum is iOS 15.1 | cat:301 (RN-B5); DOC R16 | DOC | A-cat; fact-check: pending |
| M08-F2 | Flutter: iOS 15 | cat:445 (footnote 24: F7); DOC | DOC | A-cat; fact-check: pending |
| M08-F3 | Flutter does not support several Flutter libraries in one app | cat:316 (FL-5); DOC F4 | DOC | A-cat; fact-check: pending |
| M08-N1 | Undra: schema-hash gate at load | docs/SPEC.md:138 (2.3); contract-tests/scenarios.md:394 (S16); site/blog/reads-never-cross-the-boundary (#schema-hash) | REPO | A; fact-check: pending |
| M08-N2 | iOS 15 and 16 via `ObservableObject` | adr-045 (Accepted); rev:ios-floor (Verdict, :9); docs/IOS_15_16.md; site/docs/cookbook/ios-15-16.html | REPO | A; fact-check: pending |
| M08-N3 | several cores side by side | adr-044 (Accepted); rev:abi-table (Verdict, :17-35: two cores share nothing, proven under ASan); site/docs/several-cores.html; contract-tests/scenarios.md:669 (S26) | REPO | A; fact-check: pending |
| M08-N4 | Caveat: the iOS 15 and 16 mode has not run on an iOS 15 or 16 runtime | stat checkpoint 21 ("proven by compilation and a 26.5 runtime probe, no 15/16 runtime installed"); sde:ios-floor ("no iOS 15/16 runtime here") | REPO | A; fact-check: pending |

### Class 9: Platforms and UI frameworks (rows 25 to 29, 36)

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| M09-K1 | Kotlin/Wasm is Beta | cat:180 (KMP-12); DOC K9 | DOC | A-cat; fact-check: pending |
| M09-K2 | Compose Multiplatform shares UI; desktop is Stable | cat:196 (KMP-B6), cat:447 (footnote 26); DOC K11 | DOC | A-cat; fact-check: pending |
| M09-U1 | UniFFI: Python and Ruby are first-party | cat:235 (UNI-B2); RAW U1 | RAW | A-cat; fact-check: pending |
| M09-U2 | web comes from a third-party generator | cat:221 (UNI-8); RAW U1 | RAW | A-cat; fact-check: pending |
| M09-F1 | Flutter paints one UI on six platforms | cat:326 (FL-B1); DOC F7 (iOS, Android, web, Windows, macOS, Linux) | DOC | A-cat; fact-check: pending |
| M09-F2 | Flutter is not suited to text-rich static sites | cat:317 (FL-6); DOC F5 | DOC | A-cat; fact-check: pending |
| M09-F3 | React Native: web, Windows and macOS out of tree | cat:287-288 (RN-5a, RN-5b); DOC R9 | DOC | A-cat; fact-check: pending |
| M09-N1 | Undra: web (wasm core; React, Vue, Svelte, Solid hooks) | cat:131 (E9); site/docs/web.html; runtimes/ts/@undra/runtime/package.json; stat (platform-polish: React, Vue, Svelte, Solid adapters) | REPO | A; fact-check: pending |
| M09-N2 | React Native | adr-038; docs/REACT_NATIVE.md; rev:react-native (Verdict, :11); rev:rn-adapters; stat checkpoints 7, 14 | REPO | A; fact-check: pending |
| M09-N3 | Not yet: Flutter and Dart | design:97-98, :170 (G4, "after G1 proves the fourth-host pattern"); stat checkpoint 21 (no Dart piece in flight); site/data/roadmap.json ("Flutter and Dart: decided after React Native ships") | REPO | A; fact-check: pending |
| M09-N4 | Partly: desktop | cat:152 (E30: the Kotlin runtime is plain JVM; the XCFramework has iOS slices only); site/data/roadmap.json ("Desktop targets", Later) | REPO | A; fact-check: pending |
| M09-N5 | By decision: no shared UI | README.md:191 ("Not in v1 ... shared UI of any kind"); site/data/roadmap.json (notPlanned); cat:146 (E24) | REPO | A; fact-check: pending |

### Class 10: Adopting and building (rows 31, 32)

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| M10-K1 | KMP: several ways into Xcode; its IDE plugin applies a build-phase script by default | cat:453 (footnote 32), ledger-0930 K18; DOC K14 | DOC | A-cat; fact-check: pending |
| M10-U1 | UniFFI: no end-to-end packaging: no Rust for Android, no `.aar` | cat:219 (UNI-6); RAW U2 | RAW | A-cat; fact-check: pending |
| M10-F1 | React Native joins an iOS app through CocoaPods | cat:290 (RN-7); DOC R10 | DOC | A-cat; fact-check: pending |
| M10-F2 | Flutter add-to-app has limits | cat:316 (FL-5), cat:452 (footnote 31); DOC F4 | DOC | A-cat; fact-check: pending |
| M10-N1 | Undra: `undra build` runs inside Gradle, Xcode and Vite | rev:tooling (Verdict, :18); stat checkpoint 11 | REPO | A; fact-check: pending |
| M10-N2 | SwiftPM on native iOS, no CocoaPods | cat:132 (E10: Swift package with an XCFramework, "no CocoaPods dependency"). Scoped to the native Swift runtime on purpose: React Native's iOS install does use CocoaPods (docs/REACT_NATIVE.md:28-37) | REPO | A; fact-check: pending |
| M10-N3 | Partly: `undra adopt` edits none of your project files | site/docs/cookbook/from-kmp.html ("writes a core crate and the exact steps ... and edits none of them"); crates/undra-cli/src/commands/adopt.rs:1-7; cat:149 (E27) | REPO | A; fact-check: pending |

### Class 11: Running in production (rows 13, 17, 33, 34)

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| M11-K1 | KMP: background work is platform code (WorkManager is Android-only) | cat:184 (KMP-16); DOC X2; "we found no shared abstraction" is the catalogue's reading of KMP's documentation | DOC / DER | A-cat; fact-check: pending |
| M11-K2 | the IDE plugin debugs across Swift and Kotlin | cat:193 (KMP-B3); DOC K13 | DOC | A-cat; fact-check: pending |
| M11-U1 | UniFFI: not assessed | cat:394, 398, 414, 415 (row 13 "no" has no source; rows 17, 33, 34 are "—") | REPO | A; fact-check: pending |
| M11-F1 | React Native Headless JS is Android-only | cat:289 (RN-6); DOC R11 | DOC | A-cat; fact-check: pending |
| M11-F2 | both debug in their own language with DevTools | cat:438 (footnote 17: R12, F8) | DOC / DER | A-cat; fact-check: pending |
| M11-N1 | Undra, not yet: background execution, step-debugging into Rust, crash-symbol files | adr-046 (Status: Proposed, line 3); stat checkpoint 21 ("In flight: ... prod-ops"); cat:150 (E28: no symbol files); worktree `.work/prod-ops` exists, unmerged | REPO | A; fact-check: pending |
| M11-N2 | no physical-device measurements | res:870-878, res:940-945 | REPO | A; fact-check: pending |

### Class 12: Ecosystem and track record (row 35)

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| M12-K1 | KMP: JetBrains builds it | ledger-0930 K21 (BG, confirmed); cat:456 | DOC | A-cat; fact-check: pending |
| M12-K2 | Google states support | cat:456 (footnote 35); DOC K12 (developer.android.com/kotlin/multiplatform, "officially supported by Google for sharing business logic") | DOC | A-cat; fact-check: pending |
| M12-U1 | UniFFI: used extensively in Firefox | cat:236 (UNI-B3); RAW U1 | RAW | A-cat; fact-check: pending |
| M12-U2 | its README says it is far from 1.0 | cat:205-210; RAW U1 ("a long way from a 1.0 release"); ledger-0930 U04 | RAW | A-cat; fact-check: pending |
| M12-F1 | Flutter / React Native: backed by Google, and by Meta and Expo | cat:456 (footnote 35: "R, F: Meta and Expo, Google (DER)"); cat:297 (RN-B1) | DER | A-cat; fact-check: pending |
| M12-N1 | Undra: one team, no production users | cat:69-75 ("a v1.0 from one team, with no production users") | REPO | A; fact-check: pending |
| M12-N2 | nothing on a registry | README.md:146, :160 (the install channels need the first tagged release); site/data/roadmap.json (crates.io and Maven Central: Next) | REPO | A; fact-check: pending |

## 4. What we solved, and how

### The boundary

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| B01 | Five classes carry most of the weight; each gets one measured number | editorial: the five sections that follow, each with its number | DER | A-judge; fact-check: pending |
| B02 | A boundary costs twice: once per crossing, and once for what a crossing carries | framing | DER | A-judge; fact-check: pending |
| B03 | reads never cross and a write crosses once per transaction | CLAUDE.md:16 (R5) | REPO | A; fact-check: pending |
| B04 | a write returns one change-set for each store it touched | docs/SPEC.md section 3.5, 5.5; contract-tests/scenarios.md:223 (S09) | REPO | A; fact-check: pending |
| B05 | ADR-031: the platform applies what has arrived once per frame, merged, with a bounded backlog | adr-031 (title; status Accepted) | REPO | A; fact-check: pending |
| B06 | a burst of 1,667 one-row updates to a 10,000-row list is 100,000 a second at 60 Hz | res:852 (heading) | REPO | A; fact-check: pending |
| B07 | it reaches the iOS app as one drain that costs the main thread 787 µs at the median | res:856 (iPhone 17 Pro simulator: "787 µs / 1.41 ms", "1,667 → 1 (1 drain)") | REPO | A; fact-check: pending |
| B08 | applying each update on its own is estimated at 3.4 to 3.9 ms, 20 to 24 percent of a frame, from the measured cost of one | res:856 ("3.4 ms to 3.94 ms", "20.4% to 23.7%"); res:860-862 (the runtime was not reverted: an estimate) | REPO | A; fact-check: pending |
| B09 | iPhone 17 Pro simulator on an Apple M5 Pro, so not a device | res:856 | REPO | A; fact-check: pending |
| B10 | ADR-044 turned the C ABI into one function table per core | adr-044:74-75; stat checkpoint 16 | REPO | A; fact-check: pending |
| B11 | so two cores can share a process without sharing anything | rev:abi-table (Verdict, :17-35: the C harness opens two images, `nm` shows one global per core) | REPO | A; fact-check: pending |
| B12 | the implementer's paired runs put the extra indirection at +0.6 ns on a 49.8 ns synchronous call | sde:abi-table (Measurements: median +0.6 ns over eight quiet pairs); res:630 (`boundary/call_sync/add` 49.8 ns) | REPO | A; fact-check: pending |
| B13 | the reviewer, on a loaded machine, could neither confirm nor refute that and asked for a re-measure on a quiet one | rev:abi-table:153-162 ("neither confirms nor refutes the implementer's +0.6 ns ... Re-measure the host pairs when the machine is quiet") | REPO | A; fact-check: pending |
| B14 | through the generated binding a call is about 300 ns on the iOS simulator and 3.2 to 3.5 µs in Chromium | res:892-893 ("294 to 302 ns on iOS ... 3.2 to 3.5 us on the web") | REPO | A; fact-check: pending |
| B15 | against 44 ns for the core alone | res:36 (`dispatch/call_sync/add` 43.9 ns) | REPO | A; fact-check: pending |
| B16 | that is open work | stat checkpoint 9 ("E4 — the binding call path" is a new piece); res:892-905 (E4: "the decision is the integrator's") | REPO | A; fact-check: pending |

### State over the boundary

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| S01 | A `Computed<Vec<T>>` was recomputed at every commit and sent whole | res:444-446 | REPO | A; fact-check: pending |
| S02 | one edited row in a 10,000-row list shipped 352.6 KB and took 176.5 µs | res:456 (176.5 µs, 352.6 KB) | REPO | A; fact-check: pending |
| S03 | a `DerivedList` is kept from the source's recorded operations on two order-statistic trees | res:446-450; adr-039 | REPO | A; fact-check: pending |
| S04 | ships the change: 333 ns and 158 bytes for the same edit | res:456 (333 ns, 158 bytes) | REPO | A; fact-check: pending |
| S05 | 375 ns and 158 bytes at 100,000 rows | res:457 | REPO | A; fact-check: pending |
| S06 | core side, best of three medians | res:451-453 ("`undra-signals` directly ... median per change, best of three") | REPO | A; fact-check: pending |
| S07 | a contract scenario replays 60,000 seeded operations through the Swift, Kotlin and TypeScript runtimes and checks each view's hash after every change-set | contract-tests/scenarios.md:518-526 (S19 step 9); stat checkpoint 15 ("all columns"); rev:derived-lists (L1: the RN column is open, which is why the post names three runtimes) | REPO | A; fact-check: pending |
| S08 | a platform still pays its own list copy, once per drain | res:469-471 | REPO | A; fact-check: pending |
| S09 | for a one-operation patch to a 100,000-row source, 47 µs in TypeScript and 7 µs in Kotlin | res:467 (47.3 µs, 7.2 µs "one op" at 100,000 (75,000)) | REPO | A; fact-check: pending |
| S10 | iOS 15 and 16 mode copies the list on every patch, 48 µs at 10,000 rows | site/docs/cookbook/ios-15-16.html ("48 µs for 10,000"); rev:ios-floor | REPO | A; fact-check: pending |

### The error channel

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| E01 | a failing call throws or rejects with the method's own error, the platform's cancellation, or `UndraCallError` | docs/ERRORS.md:11-19 | REPO | A; fact-check: pending |
| E02 | a closed set of five cases (cancelled by the core, panicked, refused, unavailable, malformed) | docs/ERRORS.md:21-30 | REPO | A; fact-check: pending |
| E03 | a command reports through `onError` | docs/ERRORS.md:130-139 | REPO | A; fact-check: pending |
| E04 | no reply status, cancellation or lost connection reaches `fatalError`, an uncaught exception or an unhandled rejection | docs/ERRORS.md:7-9 | REPO | A; fact-check: pending |
| E05 | a native panic is caught at the boundary and arrives as `panicked` | docs/ERRORS.md:25 (the `panicked` row); contract-tests/scenarios.md:415 | REPO | A; fact-check: pending |
| E06 | a computed that panics poisons only itself (ADR-019) | adr-019:51-80 (the amendment, A3) | REPO | A; fact-check: pending |
| E07 | a stream's failure is a typed item on the wire (ADR-036) | adr-036 (flag 2 carries `E`, flag 3 `StreamFailure`); stat checkpoint 9 | REPO | A; fact-check: pending |
| E08 | a write from a thread the runtime does not own is refused in every build (ADR-035) | adr-035:46 ("The rule holds in every build") | REPO | A; fact-check: pending |
| E09 | a wasm core cannot unwind, so it traps | docs/SPEC.md:603 (`panic=abort`; "A panic traps the instance") | REPO | A; fact-check: pending |
| E10 | with `crashRecovery()` the runtime restarts the same module and restores the last snapshot | docs/SPEC.md:603, :1408; adr-049 | REPO | A; fact-check: pending |
| E11 | 3.1 to 3.2 ms at the median from the trap to a running core, with a 100 KB state, over five runs in headless Chromium | res:603-619 (`ts/recovery_restart_100kb` p50 3.12 ms .. 3.22 ms; 101,446-byte snapshot; five runs; Chromium 153) | REPO | A; fact-check: pending |
| E12 | recovery is opt-in | docs/SPEC.md:1408 ("off by default") | REPO | A; fact-check: pending |
| E13 | a call in flight at the trap fails with `Unavailable` | docs/SPEC.md:1396 (`TransportFailure` "restarted": "a generated call sees `UndraCallError.Unavailable`") | REPO | A; fact-check: pending |

### The dev loop

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| D01 | the app reconnects by itself, with exponential backoff from 250 ms to 5 s | adr-051:35-39 (initialDelay 250 ms, maxDelay 5 s, jitter 0.5); site/docs/cli.html:113 ("reconnect by themselves, with backoff") | REPO | A; fact-check: pending |
| D02 | and observes its stores again | adr-051:51-54 | REPO | A; fact-check: pending |
| D03 | the core's state is snapshotted in memory before the rebuild and restored into the new core before it listens | site/docs/cli.html:113 ("snapshotted in this process's memory, never on disk, and restored into the new core"); adr-053 | REPO | A; fact-check: pending |
| D04 | the dev bar says "Reloaded, state kept" or why not | docs/DEV_LOOP.md:88-94 | REPO | A; fact-check: pending |
| D05 | the playground's 204 KiB snapshot restores in 189 µs | sde:dev-reload:83-84; adr-053:40-43 (209,008 bytes; `restore()` 189 us; release build, in process, best of 20) | REPO | A; fact-check: pending |
| D06 | 74 ms pass from suspending the old core to the new one listening | sde:dev-reload:81 | REPO | A; fact-check: pending |
| D07 | a page served by `undra dev` shows every store's live value and a timeline of every change-set | adr-054; sde:devtools (index entry in `.10x/decisions/sde/_index.md`); docs/DEV_LOOP.md:225-260 | REPO | A; fact-check: pending |
| D08 | a scrubber restores the core to an earlier step while the app follows | docs/DEV_LOOP.md:239 ("converges on it"), :252; rev:devtools:151-160 | REPO | A; fact-check: pending |
| D09 | it sits behind a per-run token | docs/DEV_LOOP.md:264-265; stat checkpoint 17 ("a 128-bit per-run token") | REPO | A; fact-check: pending |

### Shipping an update, and shipping the app

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| H01 | persisted data records the structure it was written with | docs/SPEC.md:162; adr-037 (title: "carries its type identity") | REPO | A; fact-check: pending |
| H02 | a changed type still reads: by name for free, through a `#[undra::migrate]` hook when a name changed | docs/SPEC.md:350; site/docs/updates.html | REPO | A; fact-check: pending |
| H03 | and never by discarding it silently | adr-037 (title: "is never discarded silently") | REPO | A; fact-check: pending |
| H04 | a queued write the new build cannot carry over becomes a dead letter the app can show | site/docs/cookbook/offline.html ("a queued write becomes a dead letter"; `outbox()`); stat checkpoint 17 ("undra-query format 2 with dead letters") | REPO | A; fact-check: pending |
| H05 | storage ports fail with typed errors (full, locked, corrupt, unavailable, io) on every platform | docs/SPEC.md:631 (`Full`, `Locked`, `Corrupt`, `Unavailable`, `Io`; "Each runtime has a failure-injection suite"); adr-049 | REPO | A; fact-check: pending |
| H06 | the hello-world web core is 116.6 KB gzipped against a 120 KB budget | bench/results/web-size.jsonl (`web/hello-wasm`: 116,575 bytes gzipped, budget 120,000); bench/budgets.toml:692-695 | REPO | A; fact-check: pending |
| H07 | the JavaScript runtime, 26 KB gzipped, sits under a 26,000-byte gate | bench/results/web-size.jsonl (`web/hello-runtime-js`: 25,996 bytes); bench/budgets.toml:707-709 (gate 26,000) | REPO | A; fact-check: pending |
| H08 | CI fails a change that exceeds either | adr-052; bench/budgets.toml:672-676; stat checkpoint 12 ("a `size` job in `bench.yml`") | REPO | A; fact-check: pending |
| H09 | the browser database adds about 299 KB gzipped, and only to an app that imports it | res:659-662 (299,165 bytes gzipped; "an app pays this only when it imports `@undra/runtime/db`") | REPO | A; fact-check: pending |

The two size figures in the post are `<!--measured:...-->` slots, filled from `bench/results/web-size.jsonl` by `build-numbers.mjs`; the task text said 116.8 KB, but the record
says 116,575 bytes (116.6 KB), and the post follows the record.

## 5. What it costs to adopt

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| A01 | The two migration guides describe three levels | site/docs/cookbook/from-kmp.html, from-uniffi.html (L1, L2, L3) | REPO | A; fact-check: pending |
| A02 | L1: a few pure functions move first, the rest of your stack stays | from-kmp.html ("L1 side by side: a few pure functions ... The KMP module, Ktor, SQLDelight and your state holders stay") | REPO | A; fact-check: pending |
| A03 | from UniFFI, plain-data functions in a new crate beside yours | from-uniffi.html ("A new Undra crate beside your UniFFI one. Plain-data functions first") | REPO | A; fact-check: pending |
| A04 | L2: one screen's state and requests become a store, a query and a mutation | from-kmp.html (L2) | REPO | A; fact-check: pending |
| A05 | L3: platform services become ports, and persistence and the offline queue move in | from-kmp.html (L3), from-uniffi.html (L3) | REPO | A; fact-check: pending |
| A06 | the core is Rust, and for an Android-first team Rust is no longer the daily language | from-kmp.html ("Rust is no longer your Android team's daily language"); cat:194 (KMP-B4, DER) | REPO / DER | A; fact-check: pending |
| A07 | a second toolchain: Rust with the iOS, Android and wasm targets, Xcode, and the Android SDK and NDK | site/blog/undra-vs-kotlin-multiplatform ("Tooling weight", ledger-0930 appendix); docs/ONBOARDING.md; README.md | REPO | A; fact-check: pending |
| A08 | `undra doctor` runs 34 checks, each with its fix | stat checkpoint 11 ("34 checks with state, observed value, exact fix"); rev:tooling | REPO | A; fact-check: pending |
| A09 | generated Swift defaults to iOS 17 | site/docs/cookbook/ios-15-16.html ("iOS 17 by default"); cat:148 (E26) | REPO | A; fact-check: pending |
| A10 | an app that supports 15 and 16 takes the `ObservableObject` shape and its extra list copy | ios-15-16.html ("A big list is copied on every patch") | REPO | A; fact-check: pending |
| A11 | an observed keyed list exists three times: in the core, a baseline copy, and each platform's mirror | site/blog/reads-never-cross-the-boundary ("What this costs"); ledger-0930 appendix (docs/SPEC.md:881, res:82-83 at the time) | REPO | A; fact-check: pending |
| A12 | `undra adopt` writes a core and the steps but edits none of your project files | from-kmp.html; crates/undra-cli/src/commands/adopt.rs:1-7 | REPO | A; fact-check: pending |

## 6. What is still open

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| O01 | Objects cannot cross as parameters or return values, and host callbacks cannot be arguments | docs/SPEC.md:951 (E0064), :925 (E0004) | REPO | A; fact-check: pending |
| O02 | a UniFFI crate meets both on day one | site/docs/cookbook/from-uniffi.html ("These are the first walls a UniFFI crate meets"); cat:33-36 | REPO | A; fact-check: pending |
| O03 | ADR-040 to 042 propose the changes and none has shipped | adr-040, adr-041, adr-042 (Status: Proposed); stat checkpoint 21 | REPO | A; fact-check: pending |
| O04 | the pagination recipe builds an infinite list on a keyed list | site/docs/cookbook/pagination.html | REPO | A; fact-check: pending |
| O05 | there is no paged query type and no interval refetch | pagination.html ("no paged query type yet"); crates/undra-query/src/lib.rs:33-36 | REPO | A; fact-check: pending |
| O06 | ADR-043 proposes both | adr-043 (Status: Proposed; title "paged queries, lazy lists and polling") | REPO | A; fact-check: pending |
| O07 | OS background execution, step-debugging into Rust and crash-symbol files are not built; ADR-046 proposes them | adr-046 (Status: Proposed) | REPO | A; fact-check: pending |
| O08 | the piece is in flight | stat checkpoint 21 ("In flight: ... prod-ops"); `.work/prod-ops` worktree | REPO | A; fact-check: pending |
| O09 | Flutter and Dart: not started; the plan was to decide after React Native, which has shipped | design:97-98, :170; stat checkpoint 7 (G1 landed); no Dart piece in stat checkpoint 21 | REPO / DER | A; fact-check: pending |
| O10 | the Kotlin runtime is plain JVM, but no desktop target ships | cat:152 (E30); site/data/roadmap.json ("Desktop targets": Later) | REPO | A; fact-check: pending |
| O11 | the CLI does not support Windows | README.md:147 ("Not supported yet: Windows") | REPO | A; fact-check: pending |
| O12 | the TypeScript runtime needs Node 20 or later | runtimes/ts/@undra/runtime/package.json:67-69 | REPO | A; fact-check: pending |
| O13 | there is no guide to sharing a core with a server | cat:483 (M-13, not scheduled); site/docs/cookbook/index.html lists no server recipe | REPO | A; fact-check: pending |
| O14 | nothing is on crates.io, Maven Central, npm or Homebrew yet; the install channels go live with the first tagged release | README.md:146, :160; site/data/roadmap.json (Distribution: Now; crates.io and Maven Central: Next) | REPO | A; fact-check: pending |
| O15 | a queued write's optimistic placeholder and its `invalidates` targets do not survive a restart | crates/undra-query/src/lib.rs:62-66 | REPO | A; fact-check: pending |
| O16 | the write replays, and the screen refetches only the mutation's own key | crates/undra-query/src/lib.rs:62-66 ("the replay invalidates just the mutation's own key") | REPO | A; fact-check: pending |
| O17 | the web database serves one tab per origin | site/docs/db.html; stat checkpoint 20 | REPO | A; fact-check: pending |
| O18 | a migration that contains its own `COMMIT` is open | stat checkpoint 20 ("Open: a migration containing its own `COMMIT`") | REPO | A; fact-check: pending |
| O19 | 3.2 to 3.9 µs through the generated TypeScript against 80 ns | res:892-895 ("3.2 to 3.5 us ... (3.5 to 3.9 us for the runtime's own `callSync`), against the blueprint's 60, 250 and 80 ns") | REPO | A; fact-check: pending |
| O20 | about 300 ns on the iOS simulator against 60 ns | res:892-895 (294 to 302 ns) | REPO | A; fact-check: pending |
| O21 | on React Native, 100,000 updates a second do not fit a frame | docs/REACT_NATIVE.md:250-258 (13 to 16 ms of every 16.7 ms frame on the iPhone 17 Pro simulator) | REPO | A; fact-check: pending |
| O22 | and the docs advise staying near 10,000 a second or below | docs/REACT_NATIVE.md:256-258 | REPO | A; fact-check: pending |
| O23 | every number is from a host, a simulator, an emulator or Chromium | res:694-700, :870-878 | REPO | A; fact-check: pending |
| O24 | the iOS 15 and 16 mode is proven by compilation and a runtime probe on iOS 26.5, not on an older runtime | stat checkpoint 21; sde:ios-floor | REPO | A; fact-check: pending |
| O25 | KMP has JetBrains and Google's stated support; UniFFI ships in Firefox | cat:69-75; DOC K12; RAW U1 | DOC / RAW | A-cat; fact-check: pending |
| O26 | Undra is one team's work with no production users | cat:69-75 | REPO | A; fact-check: pending |
| O27 | on Android, shared Kotlin is plain Kotlin with no boundary; Undra crosses JNI | cat:191 (KMP-B1: "Do not claim otherwise"); from-kmp.html ("Android has a boundary") | REPO | A; fact-check: pending |
| O28 | for an Android-first team Kotlin is the daily language, and in our judgement Rust is the adoption wall | cat:194 (KMP-B4, DER; "Rust is the adoption wall (blueprint 02)") | DER | A-judge; fact-check: pending |
| O29 | UniFFI serves Python and Ruby; Undra serves three UIs, and React Native on top | cat:235 (UNI-B2), cat:487 (M-17); RAW U1 | RAW | A-cat; fact-check: pending |
| O30 | Expo's EAS Update ships JavaScript without a new binary | cat:298 (RN-B2); DOC R14 docs.expo.dev/eas-update/introduction/ | DOC | A-cat; fact-check: pending |
| O31 | a native Undra core ships with the app; a web core updates with the site | cat:298, :488 (M-18: "Document that the web core updates with the site"); architecture (the core is a native library on iOS and Android) | DER | A-judge; fact-check: pending |
| O32 | Undra does not share UI, by decision; for one codebase on every screen, look at Flutter, React Native or Compose Multiplatform | README.md:191; cat:195-196 (KMP-B5), cat:326 (FL-B1), cat:297 (RN-B1) | REPO | A; fact-check: pending |
| O33 | writing it three times needs no new language or toolchain and keeps the best native tooling, with no boundary to debug across | cat:365-368 ("Where the status quo beats Undra") | DER | A-judge; fact-check: pending |

## 7. The default

| ID | Claim as written | Source | Basis | Checked by |
|---|---|---|---|---|
| C01 | Default is a claim about which question comes first, not about every team | editorial | DER | A-judge; fact-check: pending |
| C02 | the open rows above are few enough to check against your own requirements in an afternoon | O01-O14 and the matrix: 6 open rows, 7 partial | DER | A-judge; fact-check: pending |
| C03 | start with L1: move one function, call it from Swift, Kotlin and TypeScript | from-kmp.html ("Move one function, call it from Kotlin and Swift, ship") | REPO | A; fact-check: pending |

## 8. Links the post makes (not factual claims, listed so a reviewer can follow them)

ADR files linked on GitHub (`.10x/adrs/`): ADR-019, 031, 032, 035, 036, 037, 039, 040, 042, 043, 044, 046, 049, 051, 052, 053, 054, 055 (each exists in this tree; the GitHub
`main` URL resolves once this branch merges). Reviews: `2026-10-02-abi-table-review.md`. Records: `.10x/decisions/sde/dev-reload.md`. Benchmark anchors in `bench/RESULTS.md`
(`#the-adr-031-drain-...`, `#5-a-computed-list-...`, `#web-recovery-typescript-adr-049`, `#device-numbers-ios-android-web`): the fact-check should open each anchor once.

## 9. Left out because it could not be sourced

* "Time-travel any transaction in a devtools inspector", "the core runs in a Web Worker by default", "lazy collections", "newtypes stay typed", "the optimistic state survives app
  restarts", a Telemetry or Push port, "`undra adopt` adds the package and a bootstrap", command priorities: the blueprint claims that cat:510-516 says the code did not back on 1 October. The
  first, in a narrower form (a devtools page with a change-set timeline and restore), is now backed and is claimed; the rest are not claimed.
* The task text named ADR-046 for "crash recovery" in the shipping section. ADR-046 is Proposed and its piece is in flight; the crash recovery that shipped is ADR-049's `crashRecovery()` for
  a trapped web core, which the post attributes to ADR-049 and places in the error-channel section.
* Any claim about UniFFI on rows where the catalogue marks it "not assessed" (background execution, debugging, symbolication, device measurement).
* Crux in the matrix: the catalogue has nine Crux rows and the 36-row matrix has a Crux column, but the post's columns are KMP, UniFFI, and Flutter / React Native.
* Capacitor and "write it three times": only used as the premise's count and in one bullet of "where another tool is ahead" (cat:365-368).

## 10. Stale statements elsewhere on the site found while writing (not changed by this piece)

* `site/blog/reads-never-cross-the-boundary/index.html` ("What this costs") still says "Derived lists cross whole"; ADR-039 (derived lists) shipped. Also its sections on typed errors may predate ADR-032 amendment A.
* `site/blog/undra-vs-kotlin-multiplatform/index.html` says the Android runtime "has no remote transport yet"; ADR-051 gave Kotlin a remote transport.
* `site/data/roadmap.json` (`updated: 2026-10-01`) lists the hello-world web core at 102.7 KB, lists "A first-party inspector" under Later (devtools shipped), and puts Android adapters, React Native and Track A under "Now".
* `site/docs/db.html` and `site/docs/realtime.html` had a footer link "Queries & mutations" pointing at themselves (a `sync-chrome` drift on `main`); fixed in the separate commit that ships with this piece.

## 11. The 36-row tally behind MI02 and MI04

"1 Oct" is the catalogue's "Undra today" column (cat:382-417; row 2 "yes native, part. web" counted as yes). "Today" is the author's reading on 2026-10-02.
Counts: 1 Oct = 10 yes, 9 part., 16 no, 1 n/a. Today = 22 solved, 7 partial, 6 open, 1 n/a.

| ID | Row (cat) | 1 Oct | Today | Evidence for "today" | Basis | Checked by |
|---|---|---|---|---|---|---|
| T01 | 1 Typed domain errors across the boundary | yes | solved | docs/ERRORS.md:11-37; adr-032 | REPO | A; fact-check: pending |
| T02 | 2 A bug in shared logic does not end the app | yes | solved | adr-019:51-80 (A3); docs/ERRORS.md `panicked`; contract S17; adr-049, S22 (web); res:603-619 | REPO | A; fact-check: pending |
| T03 | 3 UI reads shared state without crossing | yes | solved | CLAUDE.md:16; contract S11 | REPO | A; fact-check: pending |
| T04 | 4 Updates cost O(change), including large lists | part. | solved | adr-027, adr-039; res:340-376, :442-480; contract S10, S19. Caveat: a plain `Computed<Vec<T>>` still crosses whole; the derived list is the O(change) path | REPO | A; fact-check: pending |
| T05 | 5 Shared state observable natively | yes | solved | cat:134 (E12); site/docs/api-swift.html, api-kotlin.html, api-typescript.html | REPO | A; fact-check: pending |
| T06 | 6 Cancellation from the UI, streams with backpressure | yes | solved | contract S06, S07; adr-036 | REPO | A; fact-check: pending |
| T07 | 7 Server-state cache with optimistic updates and rollback | yes | solved | docs/SPEC.md section 9; contract S12, S13; site/docs/queries.html | REPO | A; fact-check: pending |
| T08 | 8 Offline mutation queue that survives a restart intact | part. | partial | contract S14 (the queue replays); crates/undra-query/src/lib.rs:62-66 (optimistic state and `invalidates` do not survive) | REPO | A; fact-check: pending |
| T09 | 9 Persisted state with migrations | no | solved | adr-037; rev:persistence | REPO | A; fact-check: pending |
| T10 | 10 Paged and infinite lists, and polling | no | partial | pagination.html (a recipe on a keyed list); lib.rs:33-36 (no polling); adr-043 Proposed | REPO | A; fact-check: pending |
| T11 | 11 Structured local database (SQL) | no | solved | adr-048; site/docs/db.html; rev:ports; S25. Caveat: one tab per origin on the web | REPO | A; fact-check: pending |
| T12 | 12 WebSocket and real-time streams | no | solved | adr-047; site/docs/realtime.html; S23 | REPO | A; fact-check: pending |
| T13 | 13 OS background execution | no | open | adr-046 Proposed; stat checkpoint 21 (in flight) | REPO | A; fact-check: pending |
| T14 | 14 Default HTTP, storage and connectivity adapters on iOS, Android and web | part. | solved | rev:android-adapters; stat checkpoints 8, 14; site/docs/ports.html | REPO | A; fact-check: pending |
| T15 | 15 Live reload of shared logic on a device, keeping state | part. | solved | adr-051, adr-053; rev:dev-reload (iOS simulator, `undra` AVD, web) | REPO | A; fact-check: pending |
| T16 | 16 State inspector, transaction timeline, time travel | no | solved | adr-054; docs/DEV_LOOP.md:225-260; rev:devtools | REPO | A; fact-check: pending |
| T17 | 17 Step-debug from UI code into shared code | no | open | adr-046 Proposed (cat:478, M-8) | REPO | A; fact-check: pending |
| T18 | 18 Previews and UI tests without the real core | no | solved | adr-055; docs/TESTING.md; rev:testkit | REPO | A; fact-check: pending |
| T19 | 19 Deterministic tests (virtual time, fake I/O) | yes | solved | cat:141 (E19); adr-055 | REPO | A; fact-check: pending |
| T20 | 20 Compatibility check between bindings and library | yes | solved | docs/SPEC.md:138; contract S16 | REPO | A; fact-check: pending |
| T21 | 21 Generated Swift passes native review | yes | solved (judgement) | CLAUDE.md R3; crates/undra-bindgen/tests/golden/; adr-032, adr-045 | DER | A-judge; fact-check: pending |
| T22 | 22 Generics across the boundary | no | open | docs/SPEC.md:923 (E0002); adr-042 Proposed | REPO | A; fact-check: pending |
| T23 | 23 Objects, callbacks and listeners as arguments and returns | part. | partial | SPEC:951 (E0064), :925 (E0004); adr-040, adr-041 Proposed | REPO | A; fact-check: pending |
| T24 | 24 Supports iOS 15 and 16 | no | solved (caveat) | adr-045; rev:ios-floor; docs/IOS_15_16.md. Caveat: no iOS 15/16 runtime has run it | REPO | A; fact-check: pending |
| T25 | 25 First-party web target with a DOM UI | yes | solved | cat:131 (E9); site/docs/web.html | REPO | A; fact-check: pending |
| T26 | 26 Desktop | part. | partial | cat:152 (E30); roadmap.json (Later) | REPO | A; fact-check: pending |
| T27 | 27 Reuse of the shared code on a server | part. | partial | package.json engines `node >=20`; cat:448 (footnote 27, DER) | REPO / DER | A; fact-check: pending |
| T28 | 28 React Native UI on top | no | solved | adr-038; docs/REACT_NATIVE.md; rev:react-native; rev:rn-adapters | REPO | A; fact-check: pending |
| T29 | 29 Flutter UI on top | no | open | design:97-98, :170; no Dart piece in flight | REPO | A; fact-check: pending |
| T30 | 30 Two independent libraries built with it in one app | no | solved | adr-044; rev:abi-table; site/docs/several-cores.html; S26. Caveat: React Native runs one instance per namespace per process (docs/REACT_NATIVE.md:261-268) | REPO | A; fact-check: pending |
| T31 | 31 Incremental adoption in an existing app | part. | partial | from-kmp.html, from-uniffi.html; adopt.rs:1-7 (edits are manual) | REPO | A; fact-check: pending |
| T32 | 32 Build-system integration | no | solved | rev:tooling; stat checkpoint 11 | REPO | A; fact-check: pending |
| T33 | 33 Crash symbolication of shared code | no | open | adr-046 Proposed; cat:150 (E28) | REPO | A; fact-check: pending |
| T34 | 34 Size, startup and list performance measured on devices | part. | partial | res:694-:945 (simulator, emulator, Chromium rows; no physical device) | REPO | A; fact-check: pending |
| T35 | 35 Ecosystem, backing, production record | no | open | cat:69-75 | REPO | A; fact-check: pending |
| T36 | 36 Shared UI if the product wants it | n/a | n/a (by decision) | README.md:191 | REPO | A; fact-check: pending |
