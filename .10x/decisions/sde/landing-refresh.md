# SDE: landing refresh - the landing page, roadmap, docs index and README (2026-10-02)

Branch `wt/landing-refresh`, from `main` `0654810` (every code piece of the v1.1/v1.2 program merged,
`.10x/status.md` checkpoint 26). The numbers and wording come from the record and from the fact-checked post
"Why Undra is the default choice" (`site/blog/why-undra-is-the-default-choice/`, `claims.md`); nothing here is new
copy that the post does not support. Constraint set by the founder's taste (`docs/SITE.md`): the v1 palette, a
landing page of at most 350 words of prose, code collapsed, the live demo and the numbers cards as they are.

## What changed, before and after

| Where | Before | After | Why |
|---|---|---|---|
| Badge | "v1 shipped" | "v1 shipped · v1.x on main" | v1 is true; v1.x is merged but nothing is tagged, so the badge says "on main", never "released"; the install note under the hero (brew, npm and curl wait for the v1.0.0 tag) stays |
| Hero line | "…SwiftUI, Compose and React." | "…SwiftUI, Compose, React and React Native." (`max-width` 30ch to 36ch, balanced wrap, so it breaks in two lines) | React Native shipped (ADR-038, default adapters) |
| New strip | none | one row under the hero's call to action: New · React Native · Devtools with time travel · Derived lists · WebSocket, SSE and Db ports · Migrations · iOS 15/16 · Why Undra is the default choice → · Roadmap → (24 words) | nothing on the page said what shipped since v1; each item links its page (`docs/cli.html#undra-dev`, `concepts.html#derived-lists`, `realtime.html`, `updates.html`, `cookbook/ios-15-16.html`; React Native has no site page, so `docs/REACT_NATIVE.md` on GitHub, as the post does) |
| "How these are measured" | "Device-measured rows are not claimed here; they are on the roadmap." | "Simulator, emulator and Chromium rows are in the benchmark results; real-phone rows are not claimed and are on the roadmap." (links `bench/RESULTS.md#device-numbers-ios-android-web`) | `bench/results/device/*.json` exist; none is from a physical device |
| Cold start card | 71 µs | 85 µs | `bench/RESULTS.md` says 84.68 µs since ADR-037 (the snapshot fingerprint adds about 14 µs); 71 was the figure before it |
| Android card | 831 KB, source: README | 978.6 KB (arm64-v8a, hello world), source `bench/results/android-size.jsonl` | 831 KB was reported once, by the CLI piece, on 2026-09-30 and never re-measured (its twin, the 85 KB web size, was found false by the device-bench review). Measured now: `undra init` template, `undra build --platform android --release`, 978,552 bytes arm64-v8a and 1,045,200 x86_64 (budget 1.2 MB per ABI), rustc 1.99.0 (CI pins 1.98.1, not installed here; the record says so). Not CI-gated, like before |
| Seven harsh-conditions cards | 5.8 M/s, 6.5 M/s, 170 k/s, 22 k/s, 29 M/s, 339 k/s, soak 0 %; p99 17.9 µs (churn), 98 µs (fan-out), 803 µs (completions) | 6.2 M/s, 7.5 M/s, 182 k/s, 27.9 k/s, 30.6 M/s, 341 k/s, soak +0.14 %; p99 16.9 µs, 59.4 µs, 573 µs | they disagreed with the committed results (`bench/results/2026-09-30-*.json`, `bench/RESULTS.md` "Harsh conditions"). Now each reads its value, and the numbers in its label, from its record file (`measured: { file, path }`), so they cannot drift again. The soak label said "100 % of target"; the record says 98 to 100 % |
| Web call path card | none | none (decision below) | |
| Tests card | "4,900+", Rust 2,700, TS 1,132, Kotlin 617, Swift 512, wasm 29 (typed in the HTML) | 7,253: Rust 3,536 · TypeScript 1,856 · Kotlin 881 · Swift 870 · React Native 110 | the counts of status.md checkpoint 26, from `site/data/tests.json` through the new `build-trust.mjs` |
| Contract grid | "19 scenarios on 3 platforms, 57 / 57" (57 hand-written cells) and "Contract-tested, 54 of 54 pass" | "33 scenarios on 3 platforms, 95 / 95", 99 cells of which the two web-only scenarios are hollow on Swift and Kotlin; "95 of 95 pass" | scenarios and their platforms come from the headings of `contract-tests/scenarios.md`; the build fails if `tests.json`'s `cellsPassing` is not the number of cells they define |
| Roadmap teaser (landing) | "v1.0 is out. In flight now." with distribution, harsh-conditions benchmarks, device numbers as "Now" | "What is still open." with Now: real-phone benchmarks, JavaScript at 16 KB; Next: brew, npm, curl | "v1.0 is out" claimed a release that is not tagged; harsh-conditions benchmarks shipped; simulator rows exist |
| Collapsed code | `visible: Computed<Vec<Todo>>`; Swift `try Todos(ctx: .shared)`; React comment | `visible: DerivedList<Todo>` with its key; `try Todos() after UndraPlaygroundCore.load()`; "React and React Native" | checked against `examples/playground/core/src/{todos,remote}.rs` and `examples/playground/generated`: `Todos` has a derived `visible`, the generated init takes `ctx: UndraCore = UndraPlaygroundCore.core` (there is no `.shared`), the Queries sample matches `remote.rs`. The samples show no `load` call besides that comment |
| Section chrome | "What's in v1", "Read the digest" row, long titles | "In v1", "Everything under the pixels.", comparison cards without "Undra", no digest row | the word budget (below) |
| Receipts layout | tests 7 / grid 5, reviews 7 / app 5 | tests 5 / grid 7, reviews 7 / app 5 (a zigzag) | a 33-column grid needs the width; the ✓ in the reviews list no longer overlaps "ASan + Miri" (`justify-self: end`) |
| Roadmap page | v1.0 item "18 scenarios, 54 of 54"; "Now" = distribution; "Next" = v1.1 and v1.2; lede "v1.0 shipped on 30 September"; "Last updated 30 Sep" | v1.0: 17 scenarios, 51 of 51 (S18 came with ADR-031 on 1 Oct) and a new "since" item for the 33-scenario grid; Now ("Open items"): real-phone rows, JS runtime at 16 KB, generic functions and objects, query handles across a dev reload, Android module tests in CI; Next ("Release and reach"): the v1.0.0 channels, crates.io and Maven Central, Windows CLI, Flutter and Dart, a custom domain; the iOS 15/16 item says it was proven by compilation and a probe on iOS 26.5 only; lede says v1 was finished on 30 Sep and the tag is still to cut; "Last updated" is generated from `roadmap.json`'s `updated` (2026-10-02); "Since v1.0" gets the same shipped styling (green label, ✓) as "v1.0" | each item checked against status.md; nothing shipped under Now/Next, nothing open under Shipped |
| Docs nav | Guides 13 entries, Cookbook 11, `db.html` and `realtime.html` in no group | Start 1 · Core 6 · Data and live data 4 · Test, ship, operate 5 · Cookbook 7 · Adopting Undra 4 · Reference 4 · API overview 3 | at most 8 per group, no page outside the nav |
| Docs index | 12 cards, "not device numbers" | 18 cards (one per group's entry points), a start list for the new areas, and a numbers note that names the simulator, emulator and Chromium rows | |
| Five docs descriptions | 158 to 187 characters | at most 155 (`callbacks`, `objects`, `paging`, `polling`, `types`) | `check-links` failed on main before this branch |
| README | 4,900+ tests, 19 scenarios, 57/57, "What's in v1", "Not in v1", 71 µs, 831 KB, 12 crates | 7,253 tests (slots), 33 scenarios and 95/95 (slots), the derived-view row, 85 µs, the Android size from its record (slot), 13 crates, a "What's in it" list with one linked line each for React Native, devtools, derived lists, the three ports, migrations, the testing kit and iOS 15/16, and a "What is not done" section that is the roadmap's Now/Next | |
| Other stale copies | `getting-started.html` ("an Android .so is 831 KB") and the reads post's table ("Android core, per ABI 831 KB") | both read the Android slot | the same stale number |

## Decisions

* **The badge.** "v1 shipped · v1.x on main". Dropping "v1 shipped" would hide that the v1 line exists; adding
  "v1.1" or "v1.2" would name releases nobody can install. Three words more than before, paid for below.
* **No web call path card.** ADR-056's figure (handle call 0.47 to 0.68 µs in headless Chromium over three runs;
  `bench/results/device/2026-10-02-web-chromium-headless*.json`) is real and the post uses it, but a card needs a
  budget. The only one is the CI gate (`[web."sync_call"]` 2.2 µs, the 5x rule), which is not the design's target
  (80 ns in-thread); drawing a bar against a gate would flatter, drawing it against the target would show 5x over,
  which is what the post says ("still over its targets") and the hero numbers section should not carry unlabelled.
  No existing card is weaker than that: the Android card, the one I looked at replacing, turned out to be worth
  keeping once measured. The through-the-binding numbers stay where the post and `bench/RESULTS.md` have them.
* **No JS runtime card.** The record says 22,100 bytes at a 22,100-byte gate: a full bar, and the same bar every
  time the gate is restated. It stays a prose number in the README, filled from the record (`web-runtime-js` slot).
* **Where the counts live.** `site/data/tests.json` is the one hand-edited place (the suite counts and the number of
  passing cells, from the integrator's matrix line); `build-trust.mjs` renders the two cards and every
  `<!--trust:NAME-->` slot, including README's. Parsing `status.md` was rejected: every checkpoint commit would then
  fail the "generated files are up to date" check until someone ran `build-all`.
* **Record-backed rows** (`build-numbers.mjs`): `{file, artifact, field}` for a size, `{file, path}` plus `digits` for a
  whole-file JSON result, `{path|time}`, `{path|int}`, `{path|pct}` label tokens. The criterion rows (49.8 ns, 228 ns,
  6.3 µs, 392 ns, 2.3 µs, 85 µs) have no JSON record, only `bench/RESULTS.md`; they stay typed in `bench.json`.
* **Untouched on purpose:** "Four adversarial reviews: signals, runtime, macros, ffi" (true, merely small: 36
  `*-review.md` files exist, but "adversarial" for each is a claim `claims.md` scopes to features, so I did not
  invent a count); the thesis line; the diagram's steps and legend; "In v1" features (they are v1's).

## The 350-word budget

342 words before. Added: the badge (+3), the hero line (+2), the strip (+24). Cut: "Show the generated code" (-3),
"Loads as you scroll." (-2), the features heading and kicker (-4), the "Read the digest" row (-3), the comparison
card titles (-3), "running here" (-2), the teaser heading (-3), the tests card title (-3), "Full roadmap" (-2).
**347** (`node site/scripts/check-links.mjs --words`). The strip is 24 words, one under the brief's 25.

## Verification

* `node site/scripts/build-all.mjs` twice: the second run changes nothing. `check-links` 47 pages OK; `sync-chrome
  --check` and `sync-docs-nav --check` clean; `node --test site/scripts/decls.test.mjs` green.
* Browser (Chromium via Playwright against a local stage of `site/` with the real playground build, plus the Browser
  pane): landing, roadmap and docs index at 1280 and 375 px, dark and light, `scrollWidth == clientWidth` on all
  twelve; screenshots in the session scratchpad (`screens/`). The live demo iframe loads (the wasm core, 10 updates a
  second at rest) and "Push it" runs: 9,951 generated a second, 120 applied after merging, 0 dropped frames, no console
  errors.

## Open, for others

* `.github/workflows/site.yml` runs `npm ci` in `examples/playground/web` only. Building the playground there failed
  on this branch until `npm ci` had also run in `runtimes/ts/@undra/runtime` (the `Db` port's worker imports the
  optional peer `wa-sqlite`, which Vite resolves from the runtime's own `node_modules`). If CI has not hit it, a
  cache or a hoisting is hiding it; the live demo is what the site job deploys. Its path filters also list
  `bench/results/web-size.jsonl` but not `android-size.jsonl` or the harsh results the cards now read.
* `contract-tests/README.md` still says thirty scenarios and 86 cells (it is 33 and 95).
* `site/og/index.html` (the social card) still says "SwiftUI, Compose and React"; re-rendering `og.png` needs network
  for the fonts.
* The Android size should be re-recorded under the pinned toolchain after the Rust 1.99 bump, and could get a gate
  like the web's.
