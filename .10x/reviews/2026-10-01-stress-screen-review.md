# The playground stress screen and "Push it" (S1b) - adversarial review

**Date:** 2026-10-01 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/stress-screen` at `07c3b23` · **Read:** `CLAUDE.md` (R10, R12), ADR-031 (decisions 2, 3, 5), the design
`.10x/specs/2026-09-30-stress-bench-design.md` section 8, the SDE record `.10x/decisions/sde/stress-screen.md`,
`examples/playground/core/src/stress.rs` and its tests, `examples/playground/web/src/{stress-stats,embed-stats,url-params}.ts`,
`views/StressView.tsx`, `App.tsx`, `main.tsx` and their tests, `smoke/smoke.spec.ts`, the TS mirror's drain path
(`runtimes/ts/@undra/runtime/src/mirror.ts`, `core.ts`) and its read-your-writes tests, `site/assets/home.{js,css}`,
`site/index.html` (live demo, trust cards), `site/data/bench.json`, `docs/HIGH_FREQUENCY.md`,
`examples/playground/README.md` · **Method:** the staged site (`site/scripts/build-local.sh`, `_site/` on :8769) driven
with headless Chromium through Playwright scripts (not the interactive pane) at 1440x900 and 390x844, plus the
standalone screen at the maximum rate and under CPU throttling.

## Verdict

The live numbers are honest. Every tile and every field of the `undra-stats` message is a defined quantity from a
named source (the core's own `generated` signal as a delta, the mirror's `changeSetsReceived` / `entriesReceived` /
`entriesApplied` / `drains` counters as deltas over a two-second rolling window, the drain listener's per-drain
`durationMs` for the percentiles and the per-change-set average, a `requestAnimationFrame` loop for dropped frames),
the tiles and the message come from one snapshot per 500 ms and agreed to the digit in every reading I took, the
generator is paced by the Timer port and corrected by the Clock port with no wall clock, randomness or thread
(R12), and in headless Chromium it held 10,005 (median; 9,946-10,058) generated a second against a 10,000 target and
99,995 (98,711-101,325) against 100,000 for 32 seconds, with 60 drains a second, no dropped frame and no console
error; the spread is the frame-quantised reading of `generated`, not the generator. The one real honesty problem was
on the landing page: "Applied 120 / s · 1.2 % of 10,083 received" next to "Generated 10,000 / s" reads as a 98.8 %
loss to anyone who does not open the closed "Measured in your browser" paragraph, when it is the merge (two signals
applied once per frame, every received update folded into the value shown). That label is fixed here (M1); six Low
findings (a clock-step rounding that made one tile disagree with the landing on the same number, two notes that
omitted what they also count, the unsurfaced backlog fold at the maximum rate, a 1000k chip, a five-item key over a
four-bar chart) are fixed too. Nothing High; nothing Medium is open.

## Findings

| # | Sev | Where (at `07c3b23`) | Finding | Status |
|---|---|---|---|---|
| M1 | Medium | `site/index.html:325`, `site/assets/home.js:62` | The landing's headline counter after "Push it" was **"Applied 119 / s"** under **"Generated 9,948 / s"**, with the sub-line "1.2 % of 10,085 received". The number is right (`entriesApplied` after merging: `value` and `generated` once per frame, 2 x 60) but the label invites the reading "only 1.2 % made it". The explanation sat in a closed `<details>`. | **Fixed**: the label is "Applied after merge" and the sub-line "10,085 received, merged into 1.2 %". Landing prose 340 -> 342 of 350 words. The playground tile's note now starts "entries after merging". |
| L1 | Low | `examples/playground/web/src/stress-stats.ts:624` | `formatDuration` compared the raw microseconds to the step rounded to a tenth. A drain of exactly one clock step reaches it as `0.10000000000000853 ms x 1000 = 100.00000000000853 µs`, which is above 100 and printed as **"100 µs"**, while the landing (which gets the message's tenth-rounded 100.0) printed "< 100 µs" for the same drain. Seen in every 10k reading: tile "< 0.1 ms / 100 µs", landing "< 100 µs". | **Fixed** (round both to a tenth) + test. |
| L2 | Low | `site/assets/home.js:23` | The landing wrote the floor as "< 100 µs", the playground as "< 0.1 ms" (`formatStep`). | **Fixed**: the landing writes "0.1 ms" from 100 µs up, as the playground does. |
| L3 | Low | `views/StressView.tsx:209`, `docs/HIGH_FREQUENCY.md:157` | "Received: change-sets, one per update" also counts the generator's own write of `generated` once per 10 ms tick (and `running`), which is why Received reads 10,085 against Generated 9,948. Unexplained, the 1 % gap looks like a measurement error. | **Fixed**: note and table row say so. |
| L4 | Low | `views/StressView.tsx:218` | "Per change-set: drain time ÷ change-sets" is exactly that, but the per-change-set parse on arrival (`mirror.ts:398-421`, 120-200 ns each per ADR-031) happens inside the core's tick, outside the drain, so 32-59 ns is the drain's share, not the main thread's whole per-change-set cost. The dropped-frames tile carries the whole cost; the note did not say the split. | **Fixed**: "the parse on arrival is outside the drain". |
| L5 | Low | `views/StressView.tsx:33, 209`, `stress-stats.ts:48-53` | At the maximum rate (`rate=1000000`, URL only) the mirror's backlog bound engages (about 96,000 entries arrive per drain against a bound of 65,536; `stats().compactions` climbs) and nothing on the screen said so; the chip read "1000k/s". The generator itself was honest: 668,000-687,000 a second shown against "target 1,000,000" (each tick commits at most 100 ms of work and 100,000 wasm-to-JS crossings take about 133 ms). | **Fixed**: `StressSnapshot.compactions` (since the last reset) from `stats().compactions`; the Received tile's note says "backlog folded N times before a frame came" when it is above 0; "1M/s" chip; a sentence in `docs/HIGH_FREQUENCY.md`. Test on the real `Mirror` with a bound of 100. The message shape is unchanged. |
| L6 | Low | `site/index.html:556-557`, `site/assets/home.css:216` | The tests card's key lists five suites (Rust, TypeScript, Kotlin, Swift, wasm 29) over a bar with four segments whose widths were computed over 4,024, not the 4,053 the headline "4,000+" counts. | **Fixed**: a fifth segment (0.7 %), widths over 4,053, a fifth opacity step. Numbers agree with `README.md` on `main` (58eb311): 2,168 + 931 + 500 + 425 + 29 = 4,053; 18 x 3 = 54. |
| L7 | Low | `examples/playground/web/src/index.css` (embed layout), `site/assets/home.css` `.frame-body` | At 390 px the embedded screen's lower tile rows (drain p50/p99, per change-set, dropped frames, heap) fall below the 470 px frame and need a scroll inside the iframe. The landing's own counters carry generated, applied, p50, p99 and dropped frames, so a phone visitor misses nothing the page claims. | **Open** (layout; not a correctness issue). |
| I1 | Info | `stress-stats.ts:230-267`, `StressView.tsx:95` | `generatedPerSec` is a delta of the **mirrored** `generated` signal, which lands once per frame, so a reading is up to one frame (1,667 updates at 100k) stale at either end of a two-second window: ±0.8 %, and the 98,711-101,325 spread I saw is exactly that. The median is on target to 0.01 %; a visitor watching for a second sees it wobble by about a percent. Reading the core's counter directly would cross the boundary (R5). | Note. |
| I2 | Info | `stress-stats.ts:306-310, 405-414`, `pauseWhenHidden` | Hidden tab: `visibilitychange` pauses the frame monitor (the gap across it is not counted, unit-tested), the mirror drains from a zero-delay task, Chrome throttles the Timer port to one tick a second and the generator commits at most 100 ms of work per tick, so the Generated tile honestly shows about a tenth of the target while hidden and recovers in two seconds. A gap over 1 s is treated as a suspension and not counted (OS sleep, a minimised window where no event fires); measured under a full jam (1M/s, CPU x8) the longest frame was 133 ms, so the rule never hides a jam the page itself caused. A CDP `Page.setWebLifecycleState frozen` did not freeze headless Chromium (timers kept running, visibility stayed `visible`), so the hidden-tab path rests on the unit tests. | Note. |
| I3 | Info | `smoke/smoke.spec.ts:154-157`, `examples/playground/core/src/remote.rs:199-215`, `core.ts:361, 722`, `runtimes/ts/@undra/runtime/test/coalesce.test.ts:562-630` | The `remote-toggle` change (`check()` -> `click()` then `expect(...).toBeChecked()`) is the documented one-frame lag, not a read-your-writes regression. `set_remote_done` commits the optimistic write before its first `await` (the HTTP PATCH, answered 300 ms later), so that change-set is produced before the reply and is frame-aligned (ADR-031 decision 2, Consequences "Latency"); Playwright's `check()` asserts the state right after the click without retrying. Condition (b) holds: a reply drains before the promise settles (`queueFlush`), `callSync` flushes before it returns, both tested for success, rejection and the in-call delivery of wasm-main. | Note. |
| I4 | Info | `App.tsx:38-45`, `StressView.tsx:131-139` | `autostart=1` applies only to the view the page opened on (`choose()` clears it): coming back to the Stress tab does not restart the generator (smoke-tested). Leaving the tab stops the generator and releases the store; nothing keeps running unseen. That is the right call: a firehose running behind the Counter tab would show "running" nowhere, and a returning visitor sees "stopped" with a fresh store at 0. | Note. |
| I5 | Info | `stress-stats.ts:405-424` | Calibration under load: with the CPU throttled x8 and the generator at 100k from the first frame, the first 60 gaps were bimodal (16.7 or 133 ms) and the median stayed 16.7 ms, so dropped frames were counted against the true interval (357 in 6 s shown, 126 in the first second by my own count against 16.7 ms). No change. | Note. |
| I6 | Info | `stress.rs:214-249` | R12: the task reads `ctx.clock().monotonic_ns()` and sleeps on `ctx.sleep` (Timer port); no `std::time::Instant`/`SystemTime`, no randomness, no thread; `start` twice retunes in place (documented at :144-147, test :605-616), `stop` is idempotent (:186-188, test :580-602), the task holds a `Weak` and ends when the store is released or restored (tests :694-727), `burst` never touches `generated` (test :679-691). Fake-clock tests are exact at 1, 150, 1,000, 5,000, 10,000 and 1,000,000 a second. | Note. |

## What was checked and passed

* **Counter sources** (`stress-stats.ts:7-24`): generated = Δ`generated` signal; received = Δ`changeSetsReceived`;
  applied = Δ`entriesApplied` (every `registration.apply` call after folding, `mirror.ts:696-703`); merge ratio =
  applied / Δ`entriesReceived`, the same window; drains = Δ`drains`; p50/p99 = nearest-rank over per-drain
  `durationMs` in the two-second window (`DrainWindow`), timed inside `flush` around fold, apply and the signal
  `batch` that notifies subscribers (`mirror.ts:461-511`; React's render is outside it, as documented); per
  change-set = Σ drain ms / Σ change-sets. The clock floor is on the tile ("clock step 0.1 ms") and the landing never
  trusts a step below 100 µs (`home.js:18, 53`).
* **One snapshot** (`StressView.tsx:103-108`): `measuring.snapshot()` once per 500 ms feeds both `setSnapshot` and
  `channel.post(toStressMessage(...))`. Landing counters and iframe tiles read back to back agreed exactly at 10k and
  100k, desktop and phone.
* **Embed security** (`home.js:49`): `e.source === frame.contentWindow && e.origin === location.origin`, every new
  field through `Number()` + `isFinite`; the stress cells show only when `generatedPerSec` is a finite number. The
  iframe posts to `location.origin` (`main.tsx:18`), so a foreign embedder gets nothing. The 5 s give-up note still
  appears when no stats arrive after "Push it" (`home.js:42-45`).
* **Base demo labels**: the 10 Hz list demo's p50/p99 are now per drain (one drain = one change-set there), and the
  "Measured in your browser" paragraph and `docs/SITE.md` say so.
* **Palette**: the new counter cells use `--accent` (`#ff6a2a` / `#c2410c`) and `--text-3` only; the embedded
  screen takes the landing's orange through the theme parameter. Nothing cyan.
* **Keyboard**: tab order tabs -> mode chips -> rate chips -> Start/Stop -> Burst; chips carry `aria-pressed`, the
  groups `aria-label`, the tiles `<dl aria-label="Measured in this browser">`; Enter and Space on the chips, Start and
  Stop work.
* **Trust numbers**: 4,000+ (4,053), 18 scenarios x 3 = 54/54, S18 in the grid; `check-links` 20 pages OK on source
  and `_site/`; landing prose 342 of 350 words.
* **Docs**: `docs/HIGH_FREQUENCY.md` "See it yourself" carries the record's table, labelled as the reference
  machine's headless Chromium, "not a guarantee"; `examples/playground/README.md` describes the screen and the URL
  parameters (now including the rate range).

## Runs

Apple M5 Pro, headless Chromium (Playwright), `wasm-main`, the staged site from `_site/` on :8769, the iframe as
"Push it" opens it, then its own 100k chip. Rates are the medians of the posted messages after the window filled;
ranges are min-max.

| Where | Rate | Generated / s | Received / s | Applied / s | Drains / s | Drain p50 / p99 | ns per change-set | Dropped frames |
|---|---|---|---|---|---|---|---|---|
| landing, 1440x900, last 4 s of 12 s | 10,000 | 10,005 (9,946-10,058) | 10,087 | 120 | 60 | < 0.1 ms / 0.1-0.2 ms | 59 | 0, longest 16.8 ms |
| landing, 1440x900, 32 s | 100,000 | 99,995 (98,711-101,325) | 100,080 | 120 | 60 | < 0.1 ms / 0.5-0.8 ms | 36 | 0, longest 16.8 ms |
| landing, 390x844, last 4 s of 12 s | 10,000 | 10,002 (9,951-10,062) | 10,087 | 120 | 60 | < 0.1 ms / 0.1-0.3 ms | 70 | 0 |
| landing, 390x844, 12 s | 100,000 | 99,980 (99,573-100,734) | 100,070 | 120 | 60 | < 0.1 ms / 0.8-0.9 ms | 47 | 0 |
| standalone, 20 s | 1,000,000 (URL) | 668,000-687,000 | the same | 13-14 | 7 | 0.5-0.6 / 0.7-1.0 ms | 5-6 | 212-218 per 5 s, longest 133 ms; `evaluate` round trip up to 103 ms; Stop shown in 177 ms |
| standalone, CPU x4, 6 s | 100,000 | 100,268 | 100,235 | 94 | 47 | < 0.1 / 4.0 ms | 142 | 1 |
| standalone, CPU x8, 6 s | 100,000 | 83,434 | 83,442 | 17 | 9 | 3.6 / 6.8 ms | 336 | 357, longest 117 ms |

No console error or warning in any run. Heap read a constant 9.5 MB (headless Chromium quantises
`performance.memory`; the tile says "Chrome only").

Checks on the reviewed tree with the fixes: `cargo fmt --check` clean; `cargo clippy --workspace --all-targets -- -D
warnings` clean; `cargo test -p playground-core` 67 passed (no Rust file changed in this review); playground web
`npm test` 102 passed (one new), `npm run build` green, `npm run smoke` 4 passed; `node site/scripts/build-all.mjs`,
`check-links` (20 pages, source and `_site/`), `--words` 342 of 350. The contract suite was not re-run: the core is
untouched.
