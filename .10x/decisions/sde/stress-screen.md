# SDE - the playground stress screen and "Push it" (wt/stress-screen, 2026-09-30)

S1b of the harsh-conditions work (`.10x/specs/2026-09-30-stress-bench-design.md` section 8; the Rust
harness is S1a in `stress-bench.md`; the runtime half is `frame-coalesced-delivery.md`). No wire, ABI,
threading or generated-shape change: the playground's own bindings grew two signals and two methods,
which is app code (design 9.1: no ADR). The core's schema hash is now `0xabdf844b53e0bc10`.

## What was built

**Core** (`examples/playground/core/src/stress.rs`, `lib.rs`): `Stress` keeps `value` (0) and
`#[undra(no_coalesce)] progress` (1) and gains `generated: Signal<u64>` (2) and `running: Signal<bool>` (3),
appended so no existing signal id moves. `start(mode, per_second) -> Result<(), StressError>` and `stop()`;
`burst` is untouched and deliberately does **not** count in `generated` (contract scenario S18 asserts
`changeSetsReceived` grows by exactly 1000 for a burst of 1000). `StressError::RateOutOfRange { rate, max }`
for 0 and above `MAX_RATE` (1,000,000). The generator is a task spawned with `ctx.spawn`: it sleeps 10 ms
on the `Timer` port (`ctx.sleep`), reads `ctx.clock().monotonic_ns()`, adds `rate x elapsed` to a `u128`
carry (integer nanosecond-updates, no floats), commits `carry / 1e9` updates (one implicit transaction
each) capped at `rate / 10` (100 ms of work; the excess is forgotten, not paid back), writes `generated`
once per tick, and loops. State shared with the store is `Arc<Mutex<Generator>>` (epoch, active, mode,
rate); the task holds a `Weak` of it plus clones of the signals, never the store. So: `stop()` bumps the
epoch and the task ends at its next tick; `start()` while running retunes (no second task); a released or
restored store drops the `Arc`, the `Weak` fails and the task ends by itself (both tested); `assemble`
(the `restore` hook) forces `running` to false because a restored store has no task.
17 new unit tests under `undra::ports::fakes` (exact counts; rate x seconds; a rate that is not a multiple
of 100 carries its remainder; late timers paid from the clock; a 500 ms stall capped at 100 ms of work;
jittery ticks exact after every tick; stop/restart/retune leave one timer; release and restore end the
task; the same timeline gives byte-identical entries). Bindings regenerated for the three platforms
(`undra bindgen --docs`, `--check` clean). Docs on bound items avoid Rust intra-doc links (R3: they land
in Swift and Kotlin doc comments).

**Web** (`examples/playground/web/src`):

* `stress-stats.ts` (new, pure, injected clocks): `percentile`, `median`, `DrainWindow` (rolling window of
  drains: rates, p50/p99 per drain, ns per change-set), `RateWindow` (per-second rates of cumulative
  counters), `FrameMonitor` (rAF gaps, interval = median of the first 60 gaps, > 1.5 intervals drops
  `round(gap / interval) - 1`, pause/resume for hidden tabs, gaps over 1 s ignored as suspension, a 5 s
  recent window and the longest gap), `StressMeter` (ties the mirror's `stats()` and drain listener, the
  core's `generated` and signal-notification counters into one `StressSnapshot`), formatters
  (`formatDuration` says "< 0.1 ms" at or below the measured clock step), and the DOM glue
  (`browserFrameSource`, `pauseWhenHidden`, `browserHeapBytes`).
* `views/StressView.tsx` (new): mode (firehose/progress), rate (1k, 10k, 50k, 100k, plus a chip for a URL
  rate that is not a preset), Start/Stop, "Burst 1,000", the `value` and `progress` numbers with how many
  times each was applied (counted through `Signal.subscribe`: once per drain for the merged signal, once
  per entry for the `no_coalesce` one), eight tiles and the "Measured in your browser" sentence. The store
  is owned by the view (`useUndra(Stress)`); leaving the screen stops the generator, then the store is
  released. Changing mode or rate while running retunes the same generator.
* `embed-stats.ts` (rewritten around the drain listener): `StatsMessage` keeps its four base fields and
  gains optional stress fields (`generatedPerSec`, `entriesReceivedPerSec`, `entriesAppliedPerSec`,
  `mergeRatio`, `drainsPerSec`, `applyNsPerChangeSet`, `droppedFrames`, `droppedFramesRecent`,
  `longestFrameMs`, `heapMb` (absent outside Chrome), `valueApplies`, `progressApplies`, `mode`,
  `targetRate`, `running`, `runtime`). `StatsChannel` is the line to the parent; the stress screen claims it
  and posts from the same snapshot it renders (so the tiles and the payload always agree), the base poster
  stays quiet while it is claimed.
* `url-params.ts`: `screen=stress`, `rate=` (digits or `100k`, 1..1,000,000), `mode=`, `autostart=1`;
  `App.tsx` gets a Stress tab and applies `autostart` only to the view the page opened on; `main.tsx`
  builds the channel; `index.css` the tiles (and a compact embed layout that fits the landing's 470 px
  frame).
* Tests: `stress-stats.test.ts` (new, 47), `embed-stats.test.ts` (rewritten, 20), `url-params.test.ts`
  (extended, 21); Playwright smoke: the stress screen end to end and the embedded message.

**Landing page** (`site/`): `data/bench.json` `stressScreen: true` and the `method` sentence (the
core-side harsh numbers are unaffected by ADR-031; the platform-side apply is what it changes, and the live
demo shows it). `assets/home.js`: "Push it" loads `playground/?screen=stress&embed=1&rate=10000&
mode=firehose&autostart=1`, relabels the frame bar, and reveals two counters (Generated, Dropped frames)
plus the merge share under Applied ("1.2 % of 10,083 received"), all from the optional message fields;
text set from JS (the merge line) does not count toward the 350 words (landing prose: 340). `index.html` /
`home.css`: the two counter cells and a longer closed "Measured in your browser" paragraph. Also,
per the coordinator's request: the trust numbers (4,000+ tests: Rust 2,168, TypeScript 931, Kotlin 500,
Swift 425, wasm 29; 18 scenarios, 54 of 54, with S18 "coalesced burst" in the grid; the grid class is
renamed `gridcells` since `grid51` named 17 x 3), the roadmap's shipped item, two blog sentences and
`docs/ONBOARDING.md` that quoted 3,800 / 17 / 51.

**Docs**: `docs/HIGH_FREQUENCY.md` ("See it yourself: the playground stress screen"),
`examples/playground/README.md`, `docs/SITE.md`.

## Decisions

* **The base `undra-stats` percentiles are per drain, not per change-set.** Once the mirror merges, one
  drain stands for hundreds of change-sets; dividing its time by that count makes the apply look free and
  hides what the main thread paid per frame. `applyP50Us`/`applyP99Us` are now the duration of one drain
  (the design's protocol says the same); `applyNsPerChangeSet` carries the amortised number. For the
  10 Hz list demo one drain is one change-set, so nothing visible changed there.
* **`instrumentMirror` (wrapping `flush`/`enqueue`) is gone.** It counted a change-set when `pending`
  grew, which compaction (ADR-031 decision 3) breaks, and the runtime now has the drain listener it was
  working around. The poster and the meter use `mirror.addDrainListener`.
* **A typed error for a bad rate, not a clamp.** R6 says every error is a typed value; the screen shows
  the message. The web screen can only send valid rates, so it matters to other clients.
* **`generated` counts updates, written once per tick**, not once per update: one extra change-set per
  10 ms, 1% of the firehose at 10k/s and 0.1% at 100k/s, and it lets the UI read the core's own count.
* **`autostart` is for the opening view only** (found by the smoke test: coming back to the tab after
  leaving it started the generator again).
* **Embed layout drops the explanatory paragraph** (the landing's closed `details` has it) and shows four
  tiles a row.

## Numbers

Apple M5 Pro, headless Chromium (Playwright 1243), `wasm-main`, the staged landing page served from
`_site/`, "Push it" then the iframe's own rate and mode chips; median of the last eight `undra-stats`
messages (four seconds) of a ten-second run, one run per row:

| Updates a second | generated/s | received/s | applied/s | drains/s | drain p99 | ns per change-set | dropped frames |
|---|---|---|---|---|---|---|---|
| 10,000 firehose | 10,002 | 10,086 | 120 | 60 | 0.1 ms | 55 | 0 |
| 50,000 firehose | 49,992 | 50,100 | 119 | 60 | 1.2 ms | 123 | 0 |
| 100,000 firehose | 100,055 | 100,112 | 120 | 60 | 1.0 ms | 50 | 0 |
| 100,000 progress (`no_coalesce`) | 100,025 | 100,089 | 100,085 | 60 | 3.3 ms | 1,052 | 0 |

A 60-second run at 100,000 firehose (standalone page, a sample every 5 s): 6.0 million updates, 99,400 to
100,500 generated a second, received 99,987 to 100,142, applied 119 to 121, drain p99 0.6 to 1.1 ms, no
dropped frame, no console error. The merge ratio is 0.12 % at 100k/s firehose; the opt-out costs a drain
of 1.5 ms (p50) to 3.3 ms (p99) at the same rate and still drops no frame in this browser. The p99 values
are quantised by Chrome's 0.1 ms clock; the 10k row's "0.1 ms" is one step. Single runs, one machine, one
browser: they are what the page showed, not budgets; nothing gates on them (the core-side numbers are
gated by S1a).

## Verification

* `cargo fmt --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean;
  `cargo test --workspace`: 2,185 passed, 0 failed, 10 ignored (120 suites; the playground core has 67, 20
  in `stress`); `undra bindgen --docs --check` clean.
* Web: `npm test` 101 passed (5 files); `npm run build` (tsc + vite) green; `npm run smoke` 4 passed.
* `bash contract-tests/run-all.sh`: 54/54 (18 x 3), S18 included; the runners rebuild the core with the
  new schema.
* Native proof that the regenerated Swift and Kotlin compile: `undra build --platform ios` then
  `xcodebuild` of `PlaygroundApp` for the iPhone 17 Pro simulator (the app and its `.swiftmodule`s built);
  `undra build --platform android --release` then `./gradlew :app:assembleDebug` (BUILD SUCCESSFUL, 56
  tasks; needs `JAVA_HOME` of the brew JDK 17 and `ANDROID_HOME=/opt/homebrew/share/android-commandlinetools`,
  which `scripts/env.sh` does not export); and the gated `UNDRA_TEST_IOS=1 UNDRA_TEST_ANDROID=1 cargo test
  -p undra-cli --test platforms` (4 passed in 47 s).
* Site: `build-all` + `check-links` (20 pages, source and staged `_site/`), `--words` 340 of 350;
  `build-local.sh` served on :8768 and driven with headless Chromium (not the interactive pane): the
  landing's live section after "Push it", the counters and the iframe's tiles agreeing, light and dark.

## Findings for the integrator

1. **The playground smoke test had been failing since ADR-031 merged** (`remote-toggle`'s `check()` expects
   the optimistic change right after the click; the core's own write now arrives at the next frame). It
   failed identically on the tree before this piece's web changes. Fixed in the test (click, then wait for
   the state), which is the documented behaviour, not a regression.
2. **`site/data/roadmap.json` still lists "Frame-coalesced delivery" under Next as "Not shipped"** and
   "Harsh-conditions benchmark suite" under Now. ADR-031 is merged and the live demo uses it. The roadmap's
   wording follows `.10x/handoff.md` (an integrator file), so I left both for the integrator to move to
   Shipped with the handoff.
3. **`README.md` line 187** (the repository-layout table) still says "the 17 scenarios"; the 4,000+/18/54
   edit on `main` (58eb311) did not reach it. A separate hunk, so it merges cleanly when changed.
4. `scripts/env.sh` does not export `JAVA_HOME`/`ANDROID_HOME`; `./gradlew` finds `/usr/bin/java` (the
   macOS stub) and fails. The ONBOARDING gotchas list is the place for the two exports used above.
5. `.proof/` holds tracked PNGs that every `npm run smoke` rewrites; only the new `web-stress.png` is
   committed here. Re-shooting the others belongs with a proof refresh.
6. Heap reads a constant 9.5 MB in headless Chromium (Chrome quantises `performance.memory` unless
   launched with precise memory info); on a real Chrome it moves. The tile says "Chrome only".
7. Device screens (`StressView.swift`, `StressScreen.kt`) are the device phase; the store and bindings they
   need exist and compile.
