# SDE — `default-choice-post` (H4): "Why Undra is the default choice" (wt/default-choice-post, 2026-10-02)

Worktree `wt/default-choice-post`, from `main` `d1b35b5` (checkpoint 21). The post is
`site/blog/why-undra-is-the-default-choice/index.html`, dated 2026-10-02, author "Undra team" like the others; its claims ledger is
`claims.md` beside it (221 claims and a 36-row tally, every row "fact-check: pending" for the integrator's adversarial pass).
No code, no ADR, no state file touched. Read first: CLAUDE.md, docs/SITE.md, the four posts, `.10x/reviews/2026-09-30-blog-claims.md`
(the existing ledger; its basis codes are reused), the catalogue, the design's section 0 and Amendments A to D, status checkpoints 5 to 21,
the two migration cookbook pages.

## What the post is

Lede = the three-sentence premise; a "how to read this page" note (dated, sourced, our bias, simulator and host numbers only). Then:

1. **The matrix**: 12 classes covering the catalogue's 36 rows once each (State reaches the UI 3-6; Errors and crashes 1-2; What can cross 21-23;
   Data layer 7-10; Storage, sockets, services 11/12/14; Dev loop and tooling 15-16; Testing and previews 18-19; Compatibility and floors 20/24/30;
   Platforms and UI frameworks 25-29 + 36; Adopting and building 31-32; Running in production 13/17/33/34; Ecosystem and track record 35). Columns KMP / UniFFI /
   Flutter-React Native / Undra. Every Undra cell is "Solved: how" and/or "Not yet: what", with a link to an ADR, a docs page or a review.
2. **What we solved, and how**: five sections, one number each: the boundary (ADR-031: 1,667 one-row patches a frame arrive as one 787 µs drain on the iOS
   simulator; ADR-044: +0.6 ns, with the reviewer's caveat), state over the boundary (ADR-039: 176.5 µs / 352.6 KB to 333 ns / 158 B at 10,000 rows),
   the error channel (ADR-032/036/035/019, web recovery 3.1 to 3.2 ms), the dev loop (ADR-051/053/054: 204 KiB restored in 189 µs, 74 ms swap), shipping
   (ADR-037/049/052: 116.6 KB wasm, 26 KB JS).
3. **What it costs to adopt** (L1 to L3, linking the cookbook, and the real costs: Rust, a second toolchain, iOS 17 default, three copies of a keyed list,
   `undra adopt` edits nothing). 4. **What is still open** (not built; built with a known limit; where another tool is ahead and stays ahead). 5. **The default**
   (one paragraph, no hype).

## Decisions and deviations from the brief

* **The tally is ours.** Catalogue "Undra today" column: 10 yes, 9 part., 16 no, 1 n/a. Today by the author's reading: 22 solved, 7 partial, 6 open, 1 by decision
  (claims.md section 11, row by row). The post says so and asks for it to be checked. Desktop stays "partial" as in the catalogue, not downgraded.
* **Only shipped things are claimed.** ADR-040 to 043 and ADR-046 are Proposed and their pieces unmerged or in flight (objects-callbacks, prod-ops, ns-storage, ts-size-e4
  worktrees exist): they appear only under "not yet". The brief's "ADR-046 ... crash recovery" is ADR-049's `crashRecovery()` (shipped); ADR-046 is production
  operations (crash-symbol files, debugger path, background execution), not shipped. The post attributes recovery to ADR-049.
* **116.6 KB, not 116.8.** `bench/results/web-size.jsonl` (the record `build-numbers.mjs` reads) says 116,575 bytes gzipped; 116.8 was checkpoint 17's figure before ports.
  The two size numbers in the post are `<!--measured:...-->` slots, so they follow the record. The JS runtime is 25,996 bytes against a 26,000 gate ("26 KB").
* **333 ns, with its condition.** RESULTS.md finding 5 (core side, `undra-signals` directly, best of three) says 333 ns / 158 B; the landing card's 392 ns is the gate row through the
  runtime. The post quotes 333 ns and says "core side, best of three medians".
* **+0.6 ns is quoted with the review's verdict**: eight quiet pairs by the implementer; the reviewer, at load 23 to 74, could neither confirm nor refute and asked for a quiet-machine re-measure
  (`.10x/reviews/2026-10-02-abi-table-review.md`). The post says exactly that, and puts the real open boundary cost (3.2 to 3.9 µs web, about 300 ns iOS, E4) in the same section and in "still open".
* **Crux is not a column** (the brief's columns); the existing UniFFI and Crux post covers it, and the ledger leaves Crux's 9 rows out.
* **A competitor claim is a catalogue row.** No competitor page was re-fetched (budget); each competitor cell carries the catalogue row id and basis (RAW/DOC/DER) in the ledger,
  with "A-cat" as the checked-by. The catalogue's own warning applies: DOC rows were read through a summarising fetch tool and need a re-read of the page. Where the catalogue
  reads a page's silence, the cell says "its documentation describes no", and UniFFI's panic and checksum rows say "as we read its Swift template".
* **Matrix layout.** `table.compare.matrix` in `blog.css` (+18 lines, scoped to `.matrix`): fixed layout and column widths on desktop (a 5-column text table at the 760 px post width),
  a card per class below 760 px with each cell's column heading from `data-label`, so nothing scrolls sideways at 375 px. The first attempt (the stock `table.compare`, `min-width: 560px`)
  gave 140 px columns and clipped cells; measured at 1100 px (2,112 px tall, no inner scroll) and at 375 px (`scrollWidth` 375, no element past the viewport).

## What could not be sourced (left out of the post)

The blueprint claims the catalogue found unbacked (cat:510-516) except the devtools one, which is now backed in a narrower form (change-set timeline and restore); anything about UniFFI on
rows the catalogue marks "not assessed"; a Crux column; any Capacitor claim; a comparison of speed with any competitor (nothing was measured against them: RESULTS.md still lists it as waiting).

## Found on the way (not changed by this piece, for the integrator)

* On `main`, `check-links` failed (the iOS 15/16 cookbook description was 171 characters) and `sync-chrome --check` failed (the db and realtime footers linked to themselves). Both fixed in
  their own commit `docs(site): ...` so the checks are green; revert it if the owning pieces fix them first.
* `site/data/pending.json` still lists `docs/realtime.html` (a note from `check-links`, not a failure; status checkpoint 19 asked for it to go when ports landed).
* Stale statements in older posts and the roadmap are listed in claims.md section 10 (the reads post's "Derived lists cross whole"; the KMP post's "no remote transport on Android";
  roadmap.json's 102.7 KB web core and "A first-party inspector: Later").
* `site/scripts/stage.sh` copies all of `site/`, so `claims.md` is deployed beside the post and linked from it. It says "fact-check: pending" on every row; either keep it public after the
  adversarial pass (the ledger is the point of the page) or have the stage script skip `*.md`.

## Verification (2026-10-02, this worktree)

`node site/scripts/build-all.mjs` (blog index, feed, sitemap, search, llms regenerate; the post gets 14 min read from the computed word count); `check-links.mjs`: 41 pages OK;
`check-links.mjs --words`: 342 words (budget 350, unchanged); `sync-chrome.mjs --check`: in sync; the page at 375 px in the browser pane, light and dark, `scrollWidth` 375
(screenshot kept in the session scratchpad as `default-choice-post-375px.jpg`); at 1100 px by headless Chrome. Merged `main` at the end (`merge-base --is-ancestor main HEAD`).

## Fact-check pass (2026-10-02, adversarial; appended)

Record: `.10x/reviews/2026-10-02-default-choice-post-fact-check.md`. `main` `da4fbbe` (checkpoint 26) merged first; the `_index.md` conflict
resolved by keeping both sides.

* **Verdict: publish.** Of the draft's 221 claim rows: 182 verified, 29 corrected (8 about other tools, 21 about Undra), 10 removed; 11 claims
  added and verified. The 36-row tally re-derived: 27 solved, 6 partial, 2 open, 1 n/a (was 22 / 7 / 6 / 1 on `d1b35b5`).
* **Competitor pages re-read verbatim** (curl, 45 linked pages plus five the ledger leans on; UniFFI's whole 68-page manual for the silence
  claims). The decisions above about "A-cat" rows no longer apply: every K/U/F row now carries a quote from the 2 October copy.
* **What I decided**: generics are "partly" (named instantiations, no generic functions or objects), not solved; step-debugging is solved with
  its scope stated (LLDB tested on the iOS simulator; Android Studio and Chrome documented); the +0.6 ns stays only beside the review's −5.1 ns
  inside ±40 ns; the dev-loop restore leads with the debug core's 1.6 ms; both derived-list numbers (333 ns direct, 392 ns through the runtime)
  are given so the post agrees with the landing card; `undra doctor`'s count is dropped (it moved from 34 to 37); `claims.md` stays public and
  the post's footer links it.
* **Stale statements fixed** (section 10 of the ledger): the reads post (derived lists, the playground snippet), the KMP post (Android remote
  transport, SQL), both migration guides (what maps now), `roadmap.json`, `pending.json`. `site/index.html` untouched.
* **Checks**: `build-all` (the post reads 15 min), `check-links --words` pass with the landing at 342 words, `sync-chrome --check` in sync; the post
  at 375 px and 1100 px with no horizontal overflow (screenshots `post-375-top.jpg`, `post-375-matrix.jpg`, `post-1100-matrix.jpg` in the session
  scratchpad). The local preview server could not be declared in `.claude/launch.json` (a hook keeps the base repository's `.claude/` read-only from
  this worktree), so a `python3 -m http.server` on 127.0.0.1:8765 served `site/` for the browser pane.
