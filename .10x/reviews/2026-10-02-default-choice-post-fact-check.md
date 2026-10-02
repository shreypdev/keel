# Fact-check: "Why Undra is the default choice" (H4)

Date: 2026-10-02. Role: adversarial fact-checker. Branch `wt/default-choice-post`, drafted against `main` `d1b35b5`; `main` `da4fbbe`
(checkpoint 26: every code piece of the v1.x program merged) merged in first, so the post is checked against what shipped. Scope: the post
`site/blog/why-undra-is-the-default-choice/index.html`, its ledger `claims.md` beside it (221 claim rows and the 36-row tally), and the stale
statements elsewhere on the site that the draft found. Same shape as `2026-09-30-blog-factcheck.md`: every verdict is also in the ledger, row by
row, in a new "Fact-check" column with the `file:line` or the quoted page it rests on.

## Verdict

**Publish.** No claim about another tool was flatly wrong, but eight were imprecise in ways a maintainer would object to, and two were unfair
by omission (Swift export's `Flow` mapping; UniFFI's generic containers). The larger problem was Undra's own side: the draft was written
before five ADRs (040, 041, 042, 043, 046) and ADR-056 merged, so its matrix, its "still open" list and four of its numbers described a tree
that no longer exists. All of it is corrected; three numbers that had been rounded in the post's favour are now exact; the +0.6 ns
boundary figure stays only next to the review's −5.1 ns inside ±40 ns. The post makes no speed comparison with another tool.

## Counts

| Verdict | The draft's 221 claim rows | The 36 tally rows |
|---|---|---|
| verified | 182 | 30 |
| corrected | 29 (8 about another tool, 21 about Undra) | 6 |
| removed | 10 | 0 |
| added by the fact-check (verified the same way) | 11 | |

Tally, re-derived row by row: **27 solved, 6 partial, 2 open, 1 n/a** (the draft's 22 / 7 / 6 / 1 was right for `d1b35b5`). Moved: row 10
paged lists and polling (partial → solved, ADR-043), 13 background execution (open → solved, with the iOS simulator caveat), 17 step-debug
(open → solved: LLDB tested on the host and the iOS simulator, Android Studio and Chrome documented), 22 generics (open → **partial**: named
instantiations only), 23 objects and callbacks (partial → solved), 33 symbolication (open → solved). The 1 October baseline (10 / 9 / 16 / 1)
re-tallies exactly; it counts row 2's "yes native · part. web" as yes, which flatters the baseline, not today.

## Method

* **Competitor pages, re-read verbatim.** All 45 competitor URLs the post links, and five more the ledger leans on (UniFFI's `Helpers.swift`,
  SKIE's Flow page, Flutter DevTools, Kotlin's compile-time guide, Ktor's client engines), were fetched on 2026-10-02 with `curl` and converted to text
  locally; each claim was searched for in that copy and the quote is in the ledger. No summarising fetch was used, which removes the
  catalogue's own caveat (its DOC rows came through one). For the "its documentation describes no" claims about UniFFI, all 68 pages of its
  manual (listed through the GitHub tree API) and its README were fetched and searched for the words the claim denies (observ, cache,
  mutation, offline, persist, hot/live reload, dev server, devtools, inspector, mock/fake, generic, adapter, sqlite, websocket). The callout
  now says that silence is read as of 2 October 2026.
* **Undra claims** against `main` `da4fbbe` plus this branch: `.10x/status.md` checkpoints 5 to 26, `bench/RESULTS.md`,
  `bench/results/web-size.jsonl`, `bench/budgets.toml`, `site/data/bench.json`, ADR-019 to 056, the reviews, `docs/`, `contract-tests/scenarios.md`,
  `crates/undra-query/src/lib.rs`, the docs pages. Every number was compared digit for digit with its record.

## Findings

### About other tools (corrected)

1. **M01-K2.** "Observing a `Flow` takes SKIE or similar" left out Swift export, which "export[s] kotlinx.coroutines flows as Swift's
   AsyncSequence out of the box" (kotlinlang.org/docs/native-swift-export.html). The 2026-09-30 fact-check (its M2) raised the same omission.
   Now "SKIE or Swift export, which is Alpha".
2. **M03-U2.** "Its documented types list no generics": UniFFI's built-in type table lists `Vec<T>`, `Option<T>` and `HashMap<K, V>`. Now "its
   documentation describes no generic types of your own" (no manual page documents one).
3. **M05-K1.** "Google's Jetpack libraries are KMP-ready": Google says "Many of our Jetpack libraries" (WorkManager is not among them). Now "many".
4. **M06-K1.** "Compose Hot Reload is desktop JVM only": the page says it needs a JVM target and JetBrains is exploring others; it never says
   "only". Now "needs a desktop JVM target".
5. **M06-K2.** "Kotlin/Native build speed is a standing priority" linked a compile-time guide that does not say that. The words are JetBrains'
   August 2025 roadmap's ("a common Kotlin/Native concern"), now quoted, dated and linked.
6. **M08-K1.** "iOS 15.0 default minimum": the target page says "iOS and iPadOS 15.0 and later", no "default".
7. **M02-F2.** "Native modules send codes and messages" linked Flutter's platform-channels page; the term is React Native's. Now "Platform
   channels answer with a code and a message".
8. **M12-U2.** "Far from 1.0" is now the README's "a long way from 1.0".

Verified as written: the KMP collection copy and exception termination, Swift export's erased generics, UniFFI objects and foreign traits,
the `try!` and `rustPanic` path (link moved to `macros.swift`, where the `try!` is), the checksum `fatalError`, no end-to-end packaging, Python
and Ruby first-party, web third-party, Firefox; React Native's JS thread and dropped frames, Turbo Native Module specs, Fast Refresh's full
reload, iOS 15.1, Headless JS on Android, CocoaPods for brownfield, out-of-tree platforms, EAS Update; Flutter's gray release widget, hot
reload and native code, the previewer's missing `dart:ffi`, iOS 15, six platforms, text-rich sites, several Flutter libraries in one app;
flutter_rust_bridge's arbitrary types and closures; SQLDelight, Store, drift, TanStack Query, expo-sqlite, web_socket_channel, Ktor,
kotlinx-coroutines-test, the KMP IDE plugin's cross-language debugging, its default build-phase script, duplicated dependencies, Kotlin/Wasm
Beta, Compose desktop Stable, Google's stated support.

### About Undra (corrected, removed, added)

* **The matrix moved** with ADR-040, 041, 042, 043 and 046: "What can cross", "Data layer" and "Running in production" now say what shipped
  and how, each with its docs page (`objects.html`, `callbacks.html`, `types.html`, `paging.html`, `production.html`); generics are honestly
  "partly"; the iOS background handler's simulator caveat and the debugger path's scope (tested with LLDB on the iOS simulator, documented for
  Android Studio and Chrome) are in the cell. Row 27 (a server, partial) is now named in its cell.
* **Ten "still open" claims removed**, all made false by merged pieces: objects, callbacks, generics and newtypes "none has shipped" (3 rows
  plus "a UniFFI crate meets both on day one"), paged queries and polling (3), production operations (2), and React Native's "100,000 a second
  do not fit a frame" and "stay near 10,000" (2; ADR-056 cut the mirror's work from 21.5-22.0 ms to 5.0-5.4 ms of a 16.7 ms frame).
* **Numbers refreshed**: the hello-world wasm 116,690 B (116.7 KB, from the measured slot); the JavaScript up front 22,100 B, **exactly at its
  22,100-byte gate** (the draft said "26 KB, under a 26,000-byte gate", which was also the wrong thing measured: the gate counts the up-front
  chunk since ADR-052's amendment); Chromium's generated call 0.47 to 0.68 µs (was 3.2 to 3.5); the in-thread `callSync` 0.32 to 0.44 µs against
  its 80 ns target (the draft compared the generated call with a target that belongs to the synchronous row).
* **Rounding in the post's favour removed**: the unmerged drain "20 to 24 percent" is 20.4 to 23.7 (and 3.94 ms, not 3.9); the web recovery
  "3.1 to 3.2 ms" is 3.12 to 3.22.
* **The dev-loop number** was the release build's 189 µs; `undra dev` runs a debug core, whose restore is 1.6 ms (ADR-053's own measure). Both
  are now given, the 1.6 ms first. "The app on the simulator, the phone or the page" is now "a simulator, an emulator or a page": no physical
  phone has run it.
* **Derived lists**: the post gave 333 ns (the signals crate directly); the landing card gives 392 ns (through the runtime). Both are now in
  the post with their harnesses, so the two pages agree.
* **The ABI table's cost**: +0.6 ns stays only as "not settled", with the review's median of −5.1 ns inside a spread of ±40 ns and the owed
  quiet re-measure; the 49.8 ns it was set against predates the table and is gone from the sentence.
* `undra doctor`'s "34 checks" is now 37 declarations (prod-ops added three); the post no longer gives a count.
* "Each after an adversarial review" was true of features, not of two small fix pieces (swift-fs, dev-reload-flake): now "each feature".
* **Added, verified**: the contract grid 95/95 over 33 scenarios and the test counts of checkpoint 26 (Rust 3,536 · TS 1,856 · Kotlin 881 ·
  Swift 870 · RN 110); ADR-056's two results; panic reports to `onPanic` (ADR-046); the lazy remote transport and default ports; the generics
  limit; query handles across a reload; the iOS background caveat; the callout's "none compares Undra's speed with another tool's".

## What could not be sourced

* No quiet-machine re-measure of ADR-044's table indirection exists; the number is quoted only with its caveat.
* Step-debugging from Android Studio and Chrome is documented and checked by `undra doctor`, not run by a test; the post says "documented".
* The iOS background handler cannot run under BGTaskScheduler in the simulator (ADR-046 amendment 8); it is tested with fakes. The post says so.
* Nothing is from a physical device. The ledger's section 9 lists what was left out on purpose.

## Stale statements fixed on the way

* The reads post: "Derived lists cross whole" (now: a computed list crosses whole, a `DerivedList` ships the change) and its playground snippet
  (`visible` is a `DerivedList<Todo>` in `todos.rs`; the Kotlin and TypeScript doc comments match the generated files). `dateModified` 2026-10-02.
* The KMP post: "The Android runtime has no remote transport yet" (ADR-051 and ADR-053, with a dated correction) and "not SQLite" (the opt-in
  `Db` port, ADR-048). `dateModified` 2026-10-02.
* The two migration guides (status checkpoint 19 flagged them for this pass): "What does not map yet" listed objects, callbacks, generics and
  newtypes; it now lists generic functions and objects and other hosts, and the UniFFI mapping table has a row for callback interfaces.
* `site/data/roadmap.json`: Track A, the Android adapters, React Native and the migration guides moved to Since v1.0; the inspector left Later;
  ADR-044, 045 and 056 added; the stale 102.7 KB gone. `site/data/pending.json`: `docs/realtime.html` removed.
* `site/index.html` (the landing page) is not touched; a separate landing refresh follows.

## Checks

`node site/scripts/build-all.mjs` (blog index, feed, sitemap, search index, llms regenerate; the post now reads 15 min); `check-links.mjs --words`:
pass, landing 342 words (budget 350, unchanged); `sync-chrome.mjs --check`: in sync. The post at 375 px (matrix as cards, `scrollWidth` 375, no
element past the viewport) and at 1100 px (the table 694 px wide, no horizontal scroll) in the browser pane; screenshots in the session
scratchpad (`post-375-top.jpg`, `post-375-matrix.jpg`, `post-1100-matrix.jpg`).

## Publication

The post is dated today (2026-10-02) and appears in the blog index, the RSS feed (`pubDate` 2 Oct 2026), the sitemap, the search index and
`llms.txt`. `claims.md` stays public: `stage.sh` deploys it beside the post and the post's footer links it as "every claim and its source".
