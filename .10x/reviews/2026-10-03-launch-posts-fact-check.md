# Fact-check: the three launch posts

Date: 2026-10-03. Role: adversarial fact-checker. Branch `wt/launch-posts-fix`, cut from `origin/main` `a547226` (PR #11, "the site for launch", squash-merged; the posts as drafted
were `4cb43de` on `wt/launch-site`). Same shape as `2026-10-02-default-choice-post-fact-check.md`: every verdict is also in the post's ledger, row by row, in a new
"Fact-check (2026-10-03)" column with the `file:line` it rests on. Scope: `site/blog/undra-1-0/`, `site/blog/a-week-of-outside-use/`,
`site/blog/the-javascript-runtime-at-16-kb/` (post and `claims.md` each) and the files `build-all.mjs` regenerates from them (feed, blog index, `llms*.txt`, search index).

## Verdict

**Publish after the integrator's list below.** No number was invented, and most were exact. But 32 claims were stated without the condition that makes them true or were
stale, and four things need a decision outside these files: the web-core record is 5.6 KB too high, the test count is a floor, the repository's records quote the outside team
verbatim, and everything in "Try it in five minutes" waits for the release. All the post-level fixes are made; the ledgers say what each was.

## Counts

| Post | Ledger rows | Verified as written | Corrected (post changed) | Added (claim had no row) | Judgement, worded as such |
|---|---|---|---|---|---|
| Undra 1.0: one Rust core under native apps | 37 (34 + 3) | 23 | 8 | 3 | 3 |
| A week of outside use | 41 (38 + 3) | 24 | 14 | 3 | 0 |
| Fifteen levers: the JavaScript runtime at 16 KB | 38 (32 + 6) | 22 | 10 | 6 | 0 |

No row was removed, and none of the three posts makes a claim about a competitor's quality.

## Method

* Every number, date, count, name and comparison in each post was compared with the file the ledger cites, opened at the line, with its unit and its conditions (debug or
  release, Mac or simulator or emulator, gzipped or raw, estimate or measurement). Sources: `.10x/status.md` checkpoints 27 to 34, ADR-047, 052, 057, 060, 061, 062, the
  reviews of `sse-chunks`, `okhttp-adapters`, `bazel` and `ts-runtime-16k`, `.10x/decisions/sde/*`, `bench/budgets.toml`, `bench/results/*.jsonl`, `bench/RESULTS.md`,
  `site/data/*.json`, `contract-tests/scenarios.md`, `scripts/wasm-size.sh`, the code of `crates/undra-cli` and `runtimes/ts/@undra/runtime`, `README.md`,
  `docs/RELEASING.md`, the docs pages.
* **Re-measured.** `scripts/wasm-size.sh` on `a547226` (rustc 1.99.0, wasm-opt 133, `CARGO_NET_OFFLINE=true`): the three JavaScript rows reproduce the record exactly
  (15,774; 16,285; 39,922 = 27,064 + 12,858). The wasm row does not (finding L21).
* **Availability, checked 2026-10-03** with `gh`: `gh release list` for `shreypdev/undra` is empty and the repository has no tag; `shreypdev/homebrew-undra` and
  `shreypdev/undra-swift` do not exist; nothing is on npm or Maven Central (the records say so; not re-queried).
* Language: no exclamation mark, no hype adjective in any post (searched); terms a newcomer would not know are glossed in the 16 KB post (wire format, mirror, host, barrel,
  Vite's helper).

## Findings (what was wrong, and the fix)

| Post | Claim as drafted | Source checked | Verdict | Fix |
|---|---|---|---|---|
| 1.0 | "The hello-world web core is 118.4 KB gzipped against 120 KB" (slot `web-size`) | `bench/results/web-size.jsonl:1`; `scripts/wasm-size.sh` run; `.10x/decisions/sde/cold-restore-regression.md:72` | **stale, wrong by 5.6 KB.** The record (118,409) predates `cold-restore-regression` (#1, `773054f`, 22:11 on 2 October), which cut about 4.9 KB; `a547226` measures **112,772 B (112.8 KB)** | Post: "in the committed record, against a budget of 120 KB". **Integrator: `scripts/wasm-size.sh --record`** (slots, `README.md:54`, landing, `bench.json` row `web-size` follow) |
| 1.0 | "7,414 tests" (slot `tests-total`) | `site/data/tests.json` (as of 2 October, reload-handles); `rev:ts-runtime-16k:71` (2,030 runtime tests), `rev:sse-chunks:77` (900 Swift tests) | **a floor**: later pieces added tests; the TypeScript and Swift counts alone are already above `tests.json` | Post: "7,414 tests at the 2 October count, with more added since". **Integrator: refresh `tests.json`** |
| 1.0 | "a synchronous call ... costs 49.8 ns ... 2.3 µs" | `res:54`, `res:761`, `res:39`, `res:23`; `adr-044:102` | right, no conditions | "In a release build on the reference machine, an Apple M5 Pro and not a phone"; the 49.8 ns predates ADR-044's table (about 1 ns, not settled) |
| 1.0 | "the Android library 898.2 KB per ABI against 1.2 MB" next to two gzipped figures | `bench/results/native-size.jsonl:1-2` | unit and scope: arm64-v8a only, stripped, **not gzipped**; x86_64 is 960,776 B | "for arm64-v8a ... stripped and uncompressed ... per ABI" |
| 1.0 | "persistence that survives a schema change" | `site/docs/updates.html` (what migrates for free), ADR-037 | too wide: converts by name, a hook for the rest, refuses what it cannot convert | "migrates stored data across schema changes and refuses what it cannot convert" |
| 1.0 | `undra dev` "keeps its state across a rebuild" | `sde:launch-site` "Product findings"; `vite.ts:18-20`; `getting-started.html` | not true of a web page started with `npm run dev` (full reload, fresh state) | the exception stated in the bullet |
| 1.0 | iOS 15 and 16 "not yet on an iOS 15 or 16 device" | `README.md:225-227` ("not yet on an iOS 15 or 16 runtime") | understated the gap (no simulator runtime either) | "runtime, simulator or device" |
| 1.0 | "35 contract scenarios ... on Swift, Kotlin and TypeScript, 101 of 101" | `scenarios.md:5,20` | right; S21 and S22 are TypeScript only | "(two run on TypeScript only)" |
| 1.0 | "Try it in five minutes" | `getting-started.html` ("takes a minute or two" for the first build) | the five minutes was not timed | the first-build sentence added; flagged below |
| Week | Table: "Fixed: 7.8x on 4 KB events" under "slow on iOS"; body "In release builds" | `adr-047:191-200`; `sde:sse-chunks:49-73`; `rev:sse-chunks:78-79` | measured on a **Mac** (macOS 26.5, release harness, loopback Node server), "not verified: an iOS simulator or device"; 7.8x is the middle of three runs (6.6x to 7.9x) | cell and body say "release build, on a Mac", the harness, the three runs, 64 B unchanged, debug 4.2x and 4.3x, and the report's 2.4x inside the range |
| Week | "the parser alone went from 310 to 2,246 MB/s" | `rev:sse-chunks:70-71`; `adr-047:220-221` | no harness | "in a release harness fed 64 KiB chunks" |
| Week | "The review found two High defects ... crashed the app" | `rev:sse-chunks:42-43`; `adr-047:215-216` | defects of the fix, not of the shipped adapter; the refusal is a **behaviour change** (the old adapter streamed on a background session) | "in the fix itself", "aborted", "a behaviour change, stated in ADR-047" |
| Week | "The reported core lands at about 1.42 MB ... about 1.18 MB at `z`" | `adr-052:915-921`; `stat:587` | an **estimate** (split and scaled), ranges 1.38-1.46 and 1.14-1.21 MB; `z` reaches 1.2 MB "narrowly and not for every core of that size" | "We did not have the reported core, so we estimated"; ranges; table cell says "an estimated 1.42 MB" |
| Week | "`z` ... 1.33x slower on the simulator and 1.58x on the emulator" | `adr-052:904,912,873` | baseline missing: ratio to `"3"` on the playground's device benchmark (default 0.99x of `"3"`); not a phone | conditions added |
| Week | "`opt_level` in `undra.toml` chooses smaller or faster" | `adr-052:878-881` | which tables and values | "`[android]` or `[ios]` ... `"s"` (default), `"z"`, `"3"`"; the x86_64 and iOS gates added (960,776; 784,086 against 900 KB) |
| Week | "twelve changes ... 506 lines, the schema by 60, the diff printed 12" | `adr-062:264-265`; `schema_diff.rs:353-367`; `reviewing-generated-code.html:102` | **stale**: the fixture has thirteen changes (7 breaking, 6 additive): 581 / 77 / 13 | corrected to thirteen, 581, 77, 13 |
| Week | "reads as native to each platform's engineers" | `adr-062:27-29`; `CLAUDE.md` R3 | a quality verdict on our own output | "since the project's rules require generated code to read as a platform engineer would write it" |
| Week | OkHttp "the team's token refresh ... in their own `OkHttpClient`" | `adr-060:15-24` | paraphrase of the team's setup | "An app's ...", and the reason (the platform's client is one no app code can reach) |
| Week | "on iOS the app's `URLSession` serves all three"; tracing "appeared in no trace" fixed | `rev:okhttp-adapters:36` (M4); `rev:sse-chunks:50` (L3); `roadmap.json:117` | two limits unstated: OkHttp leaves network interceptors and the event listener out of a WebSocket upgrade; the WebSocket adapter still accepts a background session that crashes | both added, the second pointing at the roadmap |
| Week | "in a proof of concept inside its own repository" | `adr-061:17-19` | "inside its own repository" is not stated anywhere | dropped (it also says less about the team) |
| Week | ktlint "from 90 findings to none"; "Android under Bazel is declared and not yet verified" | `sde:bazel:37-39`; `examples/bazel/README.md`; `rev:bazel:38` | conditions: ktlint 1.8 on the example's Kotlin; the Android core stops at analysis (no C++ toolchain) | both added |
| Week | "Each finding we took on was a place where our tests were green and a real app was not" | `stat:576-593` | overgeneralised: true of the ceiling, the build system and the size gate, not of U5 and U7 | "Three of the five"; "their reviews" became "the five reviews" |
| 16 KB | "Nothing was removed" | `adr-057:381-383`, `218-221`, `207-216` | true of rows 1-12 only; rows 13-14 change what a production build says and names | "No feature was removed ... a production page reports an error as a code with a link"; description fields changed in four places |
| 16 KB | "The largest shares:" a table of seven | `adr-057:31-57` | false: ports (1,834) and core lifecycle (1,805) are larger than streams (945) and the bundler's 717 | "the nine largest of the 24 shares", both rows added; "a share is what code costs, not what removing it saves" |
| 16 KB | "Where the bundler emits a module is decided by re-exports" | `adr-057:75-79`, `417-420` | a property of Rolldown (Vite 8.3.1), not of bundlers | named, with "another bundler may place modules differently and save less"; the fourth thing (717 B of the bundler's own) added to "Three things" |
| 16 KB | "the review reproduced every number to the byte" | `rev:45-46` | the sizes, not the call-path timings | "the sizes" |
| 16 KB | lever table | `adr-057:279-295` | digits exact; three labels off ("remote-only", "9, 10", row 8's `snapshot`/`restore`, which the review put back up front) and the table predates the review | labels fixed, "9, 10a", row 8 and a note |
| 16 KB | "its text is `T<code>` with a link" | `adr-057:210,433` | left out the values | "a code (`T` and a number), the values and a link" |
| 16 KB | background drain "at the one moment a page cannot fetch one" | `rev:25` | absolute | "when a page is being left or is offline and may not be able to fetch one" |
| 16 KB | "4% slower on Node, from 313 to 316 nanoseconds to 325 to 333" | `rev:56-57`, `adr:353-354` | the sources say +4%; their own runs are +3.8% (best) and +4.8% (mean); no conditions | "about 4 to 5% more (four alternating runs on one machine)", `callSync` 162-168 / 162-167 |
| 16 KB | "The development build's first chunk is 21,151 bytes" | `adr-057:301-303` | measured before the review's +131 B; never re-measured | "When the levers landed ... not re-measured since the review's fixes" |
| 16 KB | "All three numbers are gated in CI; a change that grows the chunk fails the build" | `scripts/wasm-size.sh:157,252-267,299-304`; `budgets.toml:912-926` | the dev chunk is not gated; the ceiling is `min(budget, record + 5%)` (16,000; 16,600; 41,918); growth inside 226 B passes; "names the lever" only over a budget | the gate paragraph rewritten with the three ceilings and the module-list check |
| 16 KB | no conditions on "up front" | `adr-057:19-21`, `248-250` | missing: gzip level 9, a production build of the template, Vite 8.3.1, the 691 B preload helper beside the number | a paragraph and a gloss added; `wasm-main` mode and the Worker script excluded for the all-features row |

Everything else (the 69 ledger rows that read "verified", and the numbers inside the rows above) matched its source exactly, including every cell of the 16 KB lever table, ADR-057's four Highs,
the 7.8x, 2.1x, 12,600 and 84,000 to 99,000 figures, the 898,176 / 960,776 / 784,086 sizes, the 1.2 MB and 900 KB budgets, ADR-062's 130 to 250 lines, ktlint's 90, the
15,774 / 16,285 / 39,922 sizes (also re-measured), 35 scenarios and 101 cells, the Xcode 27 and GraphQL decisions, and the roadmap items the posts point at.

## Statements about other projects

Checked: Kotlin Multiplatform, UniFFI, Crux (the 1.0 post names them only as what the default-choice post weighs, and as the comparisons "say where each tool is ahead": the three
headings exist), OkHttp (one fact, measured: network interceptors and the event listener do not run on a WebSocket upgrade, `rev:okhttp-adapters:36`, `adr-060:114`),
Apollo (only that an Apollo app keeps Apollo on the platform side, `stat:591`), Bazel (what Undra's build gave it, not a judgement of it), Xcode (our machines run 26),
ktlint (a count), Vite and Rolldown (how the template's bundler places modules, with "another bundler may differ", `adr-057:417-420`). No claim about any tool's quality
remains; none was added.

## For the integrator: sentences that depend on the release existing

On 2026-10-03 there is no GitHub release or tag, no `shreypdev/homebrew-undra`, no `shreypdev/undra-swift`, nothing on npm or Maven Central. `launch-dist` will make generated
apps resolve the runtimes from GitHub at the release tag. In the **1.0 post** (`site/blog/undra-1-0/index.html`), items 1 to 6:

1. `brew install shreypdev/undra/undra` (needs the tap and the release).
2. "The installer script and cargo work too." The installer downloads the release tarball; only `cargo install --git` works today.
3. `undra init myapp` then `cd myapp/web && npm install && npm run dev`: the template pins the Undra crates at the git tag of the `undra` that made it (`init.rs:483`;
   `sde:launch-site` found the tag missing) and `@undra/runtime` from npm. Today the run needs `--undra-path`.
4. "Try it in five minutes" (heading, nav entry, the description and og fields, and the home page's promise): untimed, and impossible before 1-3.
5. The title and the first line, "Undra 1.0": the workspace is 0.1.0 and `undra --version` prints 0.1.0 until `scripts/bump-version.sh 1.0.0` and the tag (`docs/RELEASING.md:80-85`).
6. "One limit of the first days: the Swift and Kotlin runtimes are not yet published as a Swift package and on Maven Central ... `--undra-path ~/src/undra`": true now;
   `launch-dist` may change it to "resolved from GitHub at the release tag".

In the **week post** (`site/blog/a-week-of-outside-use/index.html`): 7. "Before the release, a team tried Undra for a week" assumes the release comes. The 16 KB post
describes nothing that depends on the release (its `@undra/runtime` is the package the release will publish, and is built from the checkout in every number).

## For the integrator: wrong or stale elsewhere (not edited here)

* `bench/results/web-size.jsonl` line 1 and `[size."web/hello-wasm"]`: 118,409 recorded, 112,772 measured. Re-record, then `node site/scripts/build-all.mjs`: the slot in `README.md:54`,
  the landing page, `site/data/bench.json` row `web-size` (value 118.4) and `site/docs/getting-started.html` follow. The budget's 5% ceiling is 120,000 either way.
* `site/data/tests.json` ("as of 2026-10-02, reload-handles"): the 7,414 on the landing card (`site/index.html:571`) and `README.md:76`.
* `.10x/status.md:583` (checkpoint 34) repeats the first measurement "twelve API changes are 506 generated lines, 60 schema lines, 12 diff lines"; the fixture has thirteen
  (581 / 77 / 13, `adr-062:264-265`). `.10x/status.md:570` and `557` say all-features 40,221; the record is 39,922.
* **Privacy in the repository, not in the posts.** ADR-060 lines 15-16 and ADR-061 lines 17-19 quote the outside team's feedback in its own words ("in their words"), and the posts'
  ledgers cite those files. The posts and the ledgers no longer quote it; the ADRs, `sde:sse-chunks:3`, `sde:generated-weight` and `adr-062:12` still do. The repository is public.
  Whether to paraphrase them is the integrator's call.
* `site/data/roadmap.json` "State-preserving reload (ADR-053, ADR-059): undra dev rebuilds on save and keeps the state and query handles": true of an app attached to
  `undra dev`, not of the web template under `npm run dev` (`sde:launch-site`, "Product findings").
* The `og.png` card still reads "SwiftUI, Compose and React" (`sde:launch-site`, "Left").

## What could not be verified

* "Five minutes": no timed run exists. The first build "takes a minute or two" (the docs' words).
* The SSE gains on an iPhone, the simulator or a device: `rev:sse-chunks:78-79` ("not verified"). The ledger and the post say Mac.
* The size estimate for the reported 1.6 MB core: the core itself was never available; the post says so.
* The development build's first chunk after the review's fixes (21,151 was before them): not measured here; the dev build is ungated.
* The Android sizes and the iOS slice were not rebuilt (no Android NDK build was run); the records are from after `cold-restore-regression` and `android-size`.
* npm and Maven Central were not queried; the repository's records (`README.md`, `handoff.md`, `RELEASING.md`) are the source.
* Competitor documentation was not re-fetched: the posts make no new claim about a competitor beyond the facts above.

## Checks

`node site/scripts/build-all.mjs` (feed, blog index, sitemap, search index, `llms*.txt` regenerated; the three posts and the numbers slots up to date),
`node site/scripts/check-links.mjs` (54 pages OK), `node site/scripts/check-links.mjs --words` (landing 275 of 350 words, unchanged), `node site/scripts/sync-chrome.mjs --check`
(in sync); `scripts/wasm-size.sh` as above. Post lengths after the fixes: 965, 1,241 and 1,283 words; no exclamation mark.
