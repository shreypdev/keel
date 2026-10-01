# Blog fact-check: the four launch posts

Date: 2026-09-30. Role: adversarial fact-checker. Scope: `site/blog/keel-vs-kotlin-multiplatform`,
`site/blog/keel-vs-uniffi-and-crux`, `site/blog/why-keel-keeps-your-ui-native`,
`site/blog/reads-never-cross-the-boundary`, and the author's ledger `2026-09-30-blog-claims.md`
(95 rows). Every verdict below is also recorded in the ledger's new **Verdict** column, row by row,
with the URL or `file:line` it rests on.

Method. Every row about another tool was checked against a primary source fetched today: the tool's
own documentation page, its GitHub README or template source, its docs.rs page, or its changelog.
Search-engine summaries were not used. Version-specific claims were either confirmed as of
September 2026 or reworded into architectural facts. Every claim about Keel was checked against the
worktree: `bench/RESULTS.md`, `README.md`, `docs/SPEC.md`, `examples/playground/generated`,
`runtimes/`, `crates/`, `contract-tests/`. The four posts were edited in place with minimal changes;
all four pass `xmllint --html --noout` and a tag-balance check.

## Counts

| Verdict | Rows |
|---|---|
| CONFIRMED | 68 |
| CORRECTED | 14 |
| SOFTENED | 2 |
| REMOVED | 0 |
| UNVERIFIABLE-kept-hedged | 11 |
| **Total** | **95** |

No claim about another tool was found to be flatly wrong. Fourteen were imprecise enough that a
maintainer could reasonably object, and are corrected. The one High finding is about Keel itself.

## Findings

### High: a claim that was wrong

**H1. Post 4 understated what the generated Swift does with a failure that is not the method's typed
error.** The post said the Swift wrappers stop the process on "a panic, a malformed reply or schema
drift". The generated code is broader: for a method with a typed error channel
(`throws(TodoError)`) or with no error channel at all, *every* failure that is not the typed error
reaches `keelUnexpected`, which is `fatalError`. That set includes a cancelled call, and a call
refused after shutdown or on re-entry, not only panics. Only `async throws` methods without a typed
error rethrow `KeelReplyError`. Evidence and the product consequence are under P1 below. The post
now states exactly this (post 4, "Typed errors and panic containment").

### Medium: misleading or overstated

**M1 (K08).** Post 1 said Kotlin `suspend` functions appear "as `async` in Swift with some
limitations". JetBrains' page says the `async` form "is highly experimental and has certain
limitations" (https://kotlinlang.org/docs/native-objc-interop.html). The post now says "which the
page calls experimental with limitations".

**M2 (K09, K10).** Post 1 said `Flow` has no Swift counterpart in the Objective-C mapping (true) but
did not say that Swift export handles it. JetBrains' Swift export page exports `suspend` functions
as `async` and flows as `AsyncSequence` (https://kotlinlang.org/docs/native-swift-export.html). A
KMP maintainer would read the omission as unfair. Added one sentence saying so, and the generics
limitation is now quoted precisely ("type-erased to their upper bounds").

**M3 (K11).** Post 1 paraphrased Touchlab as recommending SKIE "for production iOS UIs" and calling
Swift export "the direction things are heading". The article (Jun 12, 2026,
https://touchlab.co/the-future-of-kmps-ios-interop) says "SKIE remains the definitive, confident
choice for shipping KMP on iOS" and that "a cleaner, Objective-C-free architecture is actively
arriving". The post now uses those words.

**M4 (K15).** Post 1 said SKIE and similar libraries generate "a collector that republishes values
into something SwiftUI observes". SKIE's documentation says it "automatically converts Flows to
custom Swift classes that implement `AsyncSequence`" (https://skie.touchlab.co/features/flows);
KMP-NativeCoroutines does the same. The post now says that.

**M5 (U08).** Post 2 said UniFFI's "generated bindings check checksums when the library
initialises". Confirmed for Swift (`wrapper.swift`: `uniffiCheckApiChecksums`, `fatalError("UniFFI
API checksum mismatch...")`) and Kotlin (`NamespaceLibraryTemplate.kt`, `RuntimeException`,
skipped only with `omit_checksums`); the Python template includes its checksum block conditionally
on a checksum mode, which I did not chase. The sentence is now scoped to "its Swift and Kotlin
bindings".

**M6 (U10, U33).** Two absence claims were stated flat: UniFFI "has no notion of observable state, a
cache or a dev loop", and a Crux app "writes its own caching on top of the Http capability". Both
are now hedged as documented absences: "as far as its documentation goes", and "we found no cache
in Crux's published capabilities or in `crux_http`'s documentation as of September 2026"
(https://docs.rs/crux_http/latest/crux_http/ contains no "cache"; the README's capability list has
none).

**M7 (U30).** Post 2 said Photoroom describes "hitting exactly this limit". The article
(https://www.photoroom.com/inside-photoroom/building-live-collaboration-in-rust-for-millions-of-users-part-3,
Jan 15, 2026) frames it as the UI being unable to tell what changed when the whole project is
replaced, then key paths, then automatic diffing of two view-model instances. The post now says
"running into this with a large view model, where the UI could not tell what had changed".

**M8 (T02).** Post 3 said "Flutter draws every pixel". Flutter's overview says it uses "its own
widget set" and "bypass[es] the system UI widget libraries", but it also documents platform views
that embed native UI. Now "Flutter paints its own widgets with its own engine".

**M9 (T14).** Post 3 attributed "a semantics tree separate from its render tree" to Flutter's
architectural overview, which does not describe it. The claim is true and is now cited to
https://api.flutter.dev/flutter/semantics/SemanticsNode-class.html ("The semantics tree is
maintained during the semantics phase of the pipeline ... then uploaded into the engine for use by
assistive technology").

### Low: wording

* **L1 (K07).** "boxed primitives surface as `NSNumber` and `KotlinInt`" is now "`NSNumber`
  subclasses such as `KotlinInt`" (the page: boxes are "derived from `NSNumber`").
* **L2 (K14).** "the Kotlin/Native runtime that ships inside your app, with no serialisation step" is
  now "the Kotlin/Native runtime, which is compiled into the framework your app links, with values
  bridged to Objective-C types rather than serialised". "Kotlin/Native runtime" is JetBrains' own
  term (https://kotlinlang.org/api/core/kotlin-stdlib/kotlin.native.runtime/).
* **L3 (K30).** "a long production record" was unsourced; now "Stable since Kotlin 1.9.20 in
  November 2023" with the release notes linked (https://kotlinlang.org/docs/whatsnew1920.html).
* **L4 (U22).** The illustrative Crux snippet bound `Ok(resp)` and then called
  `resp.take_body()`, which takes `&mut self`; now `Ok(mut resp)`. `render()`,
  `Http::get(..).expect_json().build().then_send(..)`, and the `update`/`view` signatures match
  docs.rs and the Crux book.
* **L5 (U25).** "Its published capabilities are Render, Http, KeyValue and Time" is now "include ...
  at what the README calls various stages of maturity" (the README also names SSE and PubSub).
* **L6 (T13).** "iOS widgets" is now "iOS home-screen widgets ... WidgetKit views written in SwiftUI"
  with Apple's page linked.
* **L7 (K09).** "teams commonly add" (a frequency claim) is now "which is why community tooling such
  as SKIE and KMP-NativeCoroutines exists" (a fact).

### Fairness of the "When X is the better choice" sections

None was a strawman; each already argued the other tool's case in its own terms. Three honest
reasons were missing and are added:

* KMP: **hiring**. Kotlin engineers are plentiful on mobile teams and Rust engineers are not; the
  post admits this elsewhere, so the list should say it.
* UniFFI: **object-shaped APIs**. UniFFI exposes Rust objects with methods, callback interfaces and
  foreign traits, and maps async Rust to native futures (README; futures page). Keel's surface is
  stores, records, methods and ports.
* Crux: **C# shells**. The README says its FFI is "generated by BoltFFI across native, web, and C#
  shells"; Keel targets Swift, Kotlin and TypeScript only.

### Code samples labelled as another tool's API

* KMP `TodosViewModel` (post 1): idiomatic `ViewModel` + `MutableStateFlow` + `viewModelScope`;
  kept, labelled illustrative.
* UniFFI proc-macro snippet (post 2): `uniffi::setup_scaffolding!()`, `#[derive(uniffi::Record)]`,
  `#[uniffi::export]` match the README's proc-macro path; kept.
* Crux `impl App for Todos` (post 2): fixed `mut resp` (L4); signatures match crux_core 0.20's `App`
  trait, which no longer takes a capabilities argument; kept, labelled not compilable as written.

## What I could not verify, and how it is hedged

Eleven rows are judgements or architectural inferences with no primary source to check
(K12, K20, K25, U11, U14, U29, T09, T12, T16, T17, T19). Each is either in the other tool's favour,
dated "as of September 2026", or carries a hedge word ("usually", "typically", "much of", "we have
not measured"). None is a numeric or version claim.

Two sources were partial: Apple's WidgetKit page is script-rendered, so I read Apple's documentation
JSON for it (abstract and topic text: "Present your app's content in widgets with SwiftUI views");
UniFFI's Python checksum template is mode-dependent, so the checksum claim is scoped to Swift and
Kotlin (M5).

## Keel-side issues confirmed, with file:line

**P1 (High, product). The generated Swift turns contained failures into a process abort, and that
contradicts R6 for the Swift host.**

What the code does:

* `examples/playground/generated/swift/Sources/PlaygroundCore/Generated/Errors.swift:327-332`:
  `keelUnexpected(_:)` is `-> Never` and calls `fatalError("Keel: unexpected failure of a core
  call: ...")`. The comment scopes it to "a core panic, a malformed reply, or schema drift".
* `.../Generated/Stores.swift:1848-1858` (`Todos.add`, `async throws(TodoError)`): a single
  `catch` does `guard let typed = TodoError.keelFromReply(error) else { keelUnexpected(error) }`.
  `keelFromReply` (`Errors.swift:319-324`) accepts only `KeelReplyError` with `status == .error`.
  So `CancellationError`, `KeelReplyError(.cancelled)`, `KeelReplyError(.badRequest)`,
  `KeelReplyError(.panic)` and `KeelTransportError` all abort.
* `.../Generated/Stores.swift:1862-1872` (`Todos.clearDone`, sync, no error channel) and
  `.../Generated/Objects.swift:236-249` (`explode`): any thrown error aborts.
* `.../Generated/Objects.swift:252-266` (`explodeLater`, `async throws`, no typed error): rethrows;
  no abort. So the behaviour depends on whether the Rust method returns a `Result` with a typed
  error, which is the opposite of what a reader would expect.
* `runtimes/swift/KeelRuntime/Sources/KeelRuntime/Core/KeelCore.swift:228-229, 236`: cancelling the
  calling task "throws `CancellationError`" from `call`, which the typed-throws wrapper then turns
  into `fatalError`. Cancelling a SwiftUI `.task` that awaits `todos.add(title:)` is therefore a
  crash, not a cancellation. The contract suite does not exercise this path: S06 cancels
  `probe.hang()`, an untyped `async throws` method (`contract-tests/swift/Tests/ContractTests/S04_S06_Async.swift:168-177`),
  and `grep CancellationError` over the generated package finds nothing.
* The core itself issues the replies that trigger this: status 3 `cancelled` on `keel_restore` for
  calls running on replaced stores (`docs/SPEC.md:411`) and on shutdown (`:347`); status 5
  `bad_request` after shutdown (`:347`) and on re-entry (`:348`, `:462`).
* The Swift contract runner works around it on purpose: `contract-tests/swift/NOTES.md:45-47` and
  `S15_S17_Lifecycle.swift:206-214` call `KeelCore.callSync` directly for S17.1 because "the
  generated sync binding ... stops the process on purpose". So S17 proves the *boundary* contains
  the panic (`crates/keel-ffi/src/guard.rs:36`, `catch_unwind`; reply status 2 with message and
  backtrace, `docs/SPEC.md:240`; log record at level >= 4, `scenarios.md:306-318`), and then the
  generated Swift un-contains it. Kotlin (`generated/kotlin/.../Errors.kt:58-60`, `fromReply` returns
  the original `KeelReplyException`) and TypeScript (`generated/ts/src/errors.ts:21-26`) pass the
  runtime error through.

Does it contradict R6? R6 reads "Nothing escapes as a panic or an abort. Every boundary entry is
guarded; every error is a typed value." The boundary half holds: the core never aborts and answers
with a status. The second sentence does not hold for the Swift host: the panic is a typed value for
one hop and then becomes an abort in the app, by design of the generated code. For cancellation and
post-shutdown replies, which are normal outcomes the specification itself defines, the abort is not
even a "disagreement between core and bindings". I read this as contradicting R6's letter for
generated Swift and as a real crash risk in apps, and the ledger author's instinct ("sits oddly with
R6") was right.

Recommendation. This is a generated public shape, so it needs an ADR first (R11, CLAUDE.md "touching
... a generated public shape requires a new ADR"). Options, in my order of preference:

1. Give async methods a typed error that is exhaustive over what the boundary can return:
   `async throws(KeelCallError<TodoError>)` with cases `.typed(TodoError)`, `.cancelled`,
   `.core(KeelReplyError)`. Native reviewers get `catch .typed(.emptyTitle)`; nothing aborts; the
   golden files change once. Sync methods with no error channel keep `fatalError` only for a
   malformed reply or schema drift, and treat `cancelled`/`badRequest` after shutdown as a logged
   no-op (they are `Void` in every playground case I looked at; a non-`Void` sync method with no
   error channel is the one shape that still has to abort or gain a `Result`).
2. If typed throws must stay as they are: route `keelUnexpected` through a runtime-installed
   handler (`KeelCore.unexpectedFailureHandler`) whose default is `fatalError` in debug and a logged
   `KeelReplyError` in release, and special-case `CancellationError` to rethrow as
   `CancellationError` (allowed from `async throws(E)` only if `E` admits it, so this really is
   option 1 in disguise).
3. At minimum, today: add a contract step that cancels a typed-throws async method from Swift, so
   the crash is a red test and not a surprise, and document the behaviour in
   `site/docs/concepts.html` next to the panic-containment claim.

**P2 (Medium, docs). "Per-frame coalescing" is not what the runtimes do.** `site/docs/concepts.html:115`
and `docs/SPEC.md:677` say change-sets are applied "with per-frame coalescing". The code batches per
main-thread hop with no frame alignment: `runtimes/swift/KeelRuntime/Sources/KeelRuntime/Core/Mirror.swift:101-121`
(`enqueue` appends and schedules one `Task { @MainActor }` hop; `hop` flushes everything queued),
`runtimes/ts/@keel/runtime/src/mirror.ts:12,33` ("one flush per macrotask", `queueMicrotask`),
`runtimes/kotlin/.../KeelDispatchers.kt:22` (`Dispatchers.Main.immediate`). All four posts say "one
hop" and never "per frame" (`grep -ri "per-frame\|per frame" site/blog` is empty). Since the SPEC is
binding (CLAUDE.md), this is a wording correction to §11 and to the concepts page; no behaviour
changes. Suggested text: "applied on the main thread in one hop per delivered batch".

**P3 (Medium, docs). README mislabels the 49.8 ns row.** `README.md:45` says "Synchronous core call
(C ABI, end to end) 49.8 ns"; `bench/RESULTS.md:296` says "the host number above is the core half
only" and `:49-52` explain what the 49.8 ns covers. Posts 1 and 4 use the RESULTS framing ("core
side", "excluding the Swift, JNI or JavaScript side"). Fix the README label.

**P4 (Low, docs). TypeScript test count.** `README.md:55` says 897; `.10x/status.md:8, :22, :71` say
869. The posts quote only the README's "3,800+" total and "17 contract scenarios", and neither
depends on the TS number. Re-run and reconcile.

**P5 (Low, docs). `docs/blueprint.html` carries claims the posts deliberately did not reuse**, and
the posts are clean of them (grep for each phrase finds nothing under `site/blog/`):
`:296` "UniFFI underneath" (Crux; its README names BoltFFI and does not mention UniFFI),
`:297` "serialization on every call" (UniFFI; only non-trivial types go through `RustBuffer`),
`:296-297` "Own renderer (Flutter) or bridge (RN)" (React Native's docs describe host views),
`:299` and `:701` "Whole view model re-emitted on every event" (the shell pulls `view()` after a
render request), `:275` "This is what KMP cannot do". Recommend a dated banner marking the blueprint
as the pre-v1 planning document, or a refresh.

**Confirmed as stated (no change needed).** 19-function C ABI (`docs/SPEC.md:419-503`, exactly 19
`keel_*` functions; `site/docs/architecture.html:67`); envelope carries the schema hash on every
transport message (`docs/SPEC.md:187, :294`); FNV-1a 64-bit canonical hash (`:136-138`); reply status
byte (`:237`); ChangeSet layout (`:246-252`); re-entry refused with `E_REENTRANT` in every build
(`:348, :462`); ten standard ports (`:18`); retry with 20% jitter (`:569`); `#[keel(js_number)]`
opt-in for `number` (`:658`); every TS method returns a `Promise` (`:658`); recorded list operations
vs raw-write diff (`:883-884`); keyed baseline (`:881`); `keel bindgen --check` (`crates/keel-cli/src/cli.rs:85`);
16 KB-aligned Android `.so` (`crates/keel-cli/src/binary.rs:7-8`); Vue/Svelte/Solid adapters
(`runtimes/ts/@keel/runtime/src/{vue,svelte,solid}.ts`); `TestRuntime` and `FakeClock`
(`crates/keel-runtime/src/testing.rs:486`, `crates/keel-ports/src/fakes/clock.rs:61`); Swift patch
validation before mutation (`runtimes/swift/.../Wire/KeyedPatch.swift:130-131`); schema-mismatch
error names (`runtimes/swift/.../Core/Errors.swift:117`, `scenarios.md:293-297`); Swift 6 / iOS 17
baseline (`examples/playground/generated/swift/Package.swift:7,17`); XCUITest tour, Compose test
tags, Playwright on headless Chromium (`examples/playground/ios/PlaygroundAppUITests/PlaygroundTourTests.swift`,
`android/.../ui/TodosScreen.kt`, `web/playwright.config.ts:5,18`); four adversarial reviews
(`.10x/reviews/2026-09-30-keel-{ffi,macros,runtime,signals}-review.md`); "no Android remote
transport" (`site/docs/cli.html:84`); "Not in v1" (`README.md:152-153`, `docs/SPEC.md:22`); the
to-do id counter continuing above the snapshot's largest id (`examples/playground/core/src/todos.rs:87`);
all quoted playground and generated snippets match the files named in their captions.

## Files changed

* `site/blog/keel-vs-kotlin-multiplatform/index.html`: K07, K08, K09, K10, K11, K14, K15, K30;
  hiring bullet.
* `site/blog/keel-vs-uniffi-and-crux/index.html`: U08, U10, U22, U25, U30, U33; UniFFI objects bullet;
  Crux C# bullet.
* `site/blog/why-keel-keeps-your-ui-native/index.html`: T02, T13, T14.
* `site/blog/reads-never-cross-the-boundary/index.html`: the Swift failure paragraph (H1).
* `.10x/reviews/2026-09-30-blog-claims.md`: Verdict column on all 95 rows and on the Keel-side
  appendix.

Name strings ("Keel", `keel`) were left exactly as found for the separate rename pass.
