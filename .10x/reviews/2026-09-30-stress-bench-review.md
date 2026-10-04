# Harsh-conditions benchmark (S1a) - adversarial review

**Date:** 2026-09-30 · **Reviewer:** Claude Opus 5.5 (adversarial pass: measurement validity and gate strength) ·
**Piece:** `wt/stress` at `7a088a6`, review fixes committed on top · **Design:**
`.10x/specs/2026-09-30-stress-bench-design.md`; integrator decisions D1-D6 in
`.10x/decisions/architect/stress-bench.md`; implementer's record `.10x/decisions/sde/stress-bench.md`

Everything below was run on the reference machine (Apple M5 Pro, 18 cores, macOS 26.5, rustc 1.98.1), which
was **shared** throughout: load average 2 to 13 while I measured (other agents building). Each run below
gives the load in force; nothing here comes from a quiet machine.

## Verdict

The harness measures, for the most part, what it says it measures. Each timed region holds only the step
named for it, apart from the clock itself (about 35 ns of wall time per sample). The histogram resolves
4 ns at 128-255 ns. The allocation gate is sound, and the soak catches real leaks. The one claim that
was not true was the keyed-churn invariant: "the host copy never desynchronises". It compared row ids
once, at the end, so a dropped patch passed (High, fixed). Three other things were wrong or overstated:
the published churn number was explained the wrong way, the clock's quantisation and cost were not
stated, and the completions scenario's "order under contention" claim is stronger than what it tests.
All three are fixed in RESULTS.md. The CI gates are regression guards for "several times slower", not
for 2x, and they have never run on a GitHub runner (Medium, open). **The numbers may be published** as
they now stand in RESULTS.md, with fix commit `1bd19b3` merged, under these labels: "host, core side,
Apple M5 Pro, shared machine, before ADR-031". Do not advertise them as "CI-gated" until `bench.yml`
has run green on the runner at least once. Present scenario e as "8 threads completing port calls,
nothing lost", not as proof of ordering under write contention.

## Findings

| # | Sev | Where | Finding | Status |
|---|---|---|---|---|
| H1 | High | `bench/common/stress.rs:677-694` (at `7a088a6`), `bench/src/bin/soak.rs:704` (at `7a088a6`), `bench/common/host.rs:337` | The churn invariant was `host.ids() == core.ids()` checked once after the run, with no count of patches applied. An `Update` never changes an id, and the next update of the same row carries the whole row and repairs it. So **one dropped `Update` patch passed every invariant**, in release (33 k and 305 k ops) and in debug (1.2 k and 13 k ops). I demonstrated it with a fault that drops exactly the first patch. Comparing full rows alone would not have caught it either: the row had been repaired by the end of every run. | **Fixed** (`1bd19b3`): the host list is compared field for field (`Churn::rows_now`), and every operation must be applied as exactly one keyed patch (`patches == ops && fulls == 1`), in scenario b and in the soak. New `Fault::DropOneUpdate`, `ListMirror::skip_nth` and test `a_single_dropped_update_fails_the_mirror_invariants`. The count invariant fails at the 200 ms and 2 s runs, release and debug. Wired into the soak (temporarily), it fails attempt 1 with exit 1 and is not retried. |
| M1 | Medium | `bench/budgets.toml` (layer A and `[stress.*]`), `bench/RESULTS.md` "Harsh conditions" | **A 2x regression does not fail any gate.** I made the commit path 1.8x slower (a spin in `undra_signals::txn::commit`, `txn_x1000` went from 79 to 141 us). Every layer A row passed, with margins of 2.2x to 4.6x, and every sustained gate passed (firehose 4.09 M/s against a floor of 1.1 M/s, p99 295 ns against 1.1 us). The baselines also came from loaded runs (load average 3-8) rather than the quiet machine the design asked for (§6 "re-baseline on a quiet machine"), which makes them looser still. This is the file's 5x rule working as designed, but R9's "regressions fail CI" holds only for several-fold regressions. | **Open.** Said plainly in RESULTS.md now. Proposed change: after the first green CI runs, record the runner's own `stress_baseline` and tighten the layer B floors to runner-measured/2 and the p99 ceilings to 3x, keeping `UNDRA_BENCH_SCALE` for other runner classes. Or add machine-independent ratio gates, which survive a slower runner: firehose `call_set` / `txn_x1000` per transaction, churn per op / firehose per txn. |
| M2 | Medium | `.github/workflows/bench.yml:44-55` | **CI has never run these steps.** `ubuntu-latest` has 2 to 4 vCPUs. The completions scenario runs 11 threads (a spinning issuer, 8 completers, core, drain) and the soak runs 15. The completions floor and p99 ceiling are coupled (a closed loop of 256: 69 k/s means a mean latency of 3.7 ms, which is the p99 ceiling). The soak fails any second below 50% of target. Whether a runner passes is unknown. D5's "about a minute" is also low: each new step adds a fat-LTO release link (10-12 s here, several times that on a runner) plus 15-45 s of stress and 10-25 s of soak, so roughly 2-4 minutes. | **Open.** Run the workflow once (a PR or `workflow_dispatch`) before the landing page says "CI-gated". If the soak's rate gate or completions trips on the runner, set `UNDRA_BENCH_SCALE` and lower the soak's completion rate in the workflow, not in the file. |
| M3 | Medium | `bench/common/stress.rs:1006` (`completions`), `crates/undra-runtime/src/runtime.rs:1934-1938` | Scenario e claimed to show that "the core lock and the per-store delivery lock keep order under contention". `port_reply` never takes the core lock. With `core_threads: 1`, every `Fetcher` commit runs on the one `undra-core` thread, so no two threads ever commit to the same store, in this scenario or in the soak. The order invariant is still a useful regression check (core delivery and the harness queue), but it is not contended. What the 8 threads do contend is the port-completion path, which the "nothing lost" invariants cover well. | **Fixed (claim)**: RESULTS.md row e now says what is proved. **Open (scenario)**: to test ordering under contention, add a host thread that writes the `Fetcher` store through `call_sync` (a sync `bump`) while the async completions commit on the core thread, so that two threads commit one store. |
| M4 | Medium | `bench/src/bin/soak.rs:570-595` | The soak's "drift" gate fails only when the worst post-warm-up second's p99 is over 3x the median post-warm-up second. That catches a spike, not drift. A p99 that doubles steadily passes: with 6 windows the median is about 1.5x the start and the worst 2x, a ratio of 1.33. In my first 10 s soak the p99 went from 32-47 us (seconds 1-4) to 69-108 us (seconds 5-10) and the gate passed, because the warm-up excludes exactly the seconds it would have been compared with (load 4.5; a 60 s run straight after was flat at about 40 us, so that jump was the machine, which is why a real drift gate is hard on a shared host). | **Fixed (claim)**: the soak docs and RESULTS.md call it a spike gate and say a steady climb passes. **Open**: a real drift gate (the median p99 of the last quarter of windows at most 1.5x that of the first post-warm-up quarter) belongs on a quiet or dedicated runner with a 60 s soak, not on a shared 10 s CI run. |
| M5 | Medium | `bench/common/stress.rs:548` (`time_ops`), `bench/RESULTS.md` Method | The clock was not accounted for. On this host an empty timed step runs at 28.3 M/s, so each sample costs **35 ns of wall time**, which the sustained throughput includes: about a fifth of a firehose op. The firehose's 5.8 M/s is the rate with the clock in the loop; layer A's batched timing gives 125 ns, about 8 M/s. Sub-microsecond percentiles are whole 41.67 ns ticks: the published p99 of 211 ns is "5 ticks, ± 1". The histogram is not the limit (4 ns buckets at that size); the clock is. | **Fixed**: disclosed in RESULTS.md Method. The numbers are conservative, not inflated. |
| M6 | Medium | `.10x/decisions/sde/stress-bench.md` deviation 3, `bench/RESULTS.md` row b | The churn drop from 280 k to 169 k was explained as "the fixed cycle is a harder mix". The design probe's random 20/20/20/40 mix has the same proportions as the fixed cycle. The difference is the host: the probe used `CountingHost`, while the scenario decodes and applies every patch to a 10,000-row host list inside the timed step. Side by side on this build (load about 9): **243 k ops/s with the counting host, 151 k/s with the applying host**. RESULTS.md did not reconcile the design's landing-page line ("280 k list operations/s"). | **Fixed** in RESULTS.md (a paragraph after the table). The SDE record is the implementer's and is left as written; this review corrects it. |
| L1 | Low | `bench/common/stress.rs:818` | The fan-out "p50 over 100 k observed ≤ 4 x p50 over 10 k" is a timing comparison filed as an invariant. It was never retried and was asserted even in the debug smoke under `cargo test --workspace`. Under load I saw ratios up to 2.4. | **Fixed**: `Invariant::timing`; it is a gate failure (retried, skipped by the smoke), `broken()` excludes it, and `timing_failures()` reports it. |
| L2 | Low | `bench/tests/stress.rs` (gate loop), `bench/src/bin/soak.rs:828` | A pass on a retry showed only as "ok (attempt N)" on its row. The soak's final line said "soak PASSED" whichever attempt passed. | **Fixed**: the stress gate prints "passed only on a retry: ..." at the end, and the soak prints "passed on attempt N of M". Confirmed that only noisy gates are retried: stress breaks out on `broken()`, and the soak returns 1 when `invariant_broken` (checked with a real breach). |
| L3 | Low | `bench/src/bin/soak.rs:718` | The soak said "the stream is never more than one item ahead of its credit" but checked once, after the run. Scenario d does check after every round, single-threaded. | **Fixed (wording)** in the soak output, docs and RESULTS.md. **Open (optional)**: a per-second check is sound only with release ordering on `Numbers`' `produced` increment, acquire on its read, and a granted-credit counter stored before `stream_credit`; then `produced ≤ granted + 1` at every sample. |
| L4 | Low | `bench/common/stress.rs:548`, `:1006` | No layer B scenario excludes a warm-up. That is negligible at 10 s. At CI's 2 s the completions histogram includes thread start-up and the first cold calls. | **Open**: drop the first `min(10%, 200 ms)` of samples and time from throughput. This changes the definition of the published numbers, so it should be re-measured, not patched in. |
| L5 | Low | `bench/src/bin/soak.rs:828` (`main`) | `--attempts` re-runs inside the same process, so attempt 2 inherits attempt 1's heap: with an injected leak, RSS started at 12.3 MB instead of 9.8 MB. A 34 KB/s leak still failed both attempts (+1.39%, +1.75%), so I found no masking at that size. Re-executing the binary per attempt would remove the question. | **Open.** |
| L6 | Low | `bench/src/rss.rs:59` | On macOS, `ps -o rss=` excludes compressed pages, so under memory pressure a written-once leak can be compressed away and RSS stays flat. `phys_footprint` is the right metric but needs FFI (`proc_pid_rusage`), and R2 keeps that out of the bench crate. | **Open**: see "A stricter memory check" below. |
| L7 | Low | `bench/src/budget.rs:154` | `bytes_per_op` is called "exact" but is a ceiling, so fewer bytes pass. Firehose, event, fan-out and fan-out-across-stores have exact checks in code; churn and completions do not. The new patch-count invariant covers the case that mattered for churn (lost content). | **Open** (low risk). |
| L8 | Low | `bench/RESULTS.md` table, `bench/budgets.toml` `measured_*`, SDE record table | Three sources give different numbers for the same scenario (for example event at 6.5 M/s, 7.28 M/s and 7.28 M/s). That is disclosed ("best of three for the gates, the last run for the medians"), but `UNDRA_STRESS_JSON` emits whichever run produced it, and that is what the site will show. | **Open**: commit the JSON of the run you publish, with its command, date and load average, so every landing-page number traces to a file. |

High/Medium fixed: H1, and the claims or wording of M3, M4, M5 and M6. Open: M1, M2, the M3 scenario, the
M4 drift gate. Low fixes made directly: 3 (L1, L2, L3). Low open: L4 to L8.

## A stricter memory check (question 4)

The half-run warm-up is justified by data, and it does not hide per-operation leaks at the soak's rates:

* My 60 s run (load 4.7 to 10): RSS 10.36 MB at 1 s, 10.50 MB at 5 s, steps at 12, 15 and 35 s, ending at
  10.61 MB. The steps at 15 and 35 s fall inside a 20% warm-up's window. With the half-run warm-up the
  result is +0.30% (30 KB), under the 64 KiB floor.
* Sensitivity at CI's 10 s: the gate fails above both 1% (about 106 KB) and 64 KiB over a 5 s window,
  about 21 KB/s. At about 170 k operations a second that is 0.13 bytes per operation. The 60 s soak
  catches about 3.5 KB/s.
* Injected into `undra_signals::txn::commit` (never committed): **100 bytes per commit** failed at +90.6%
  and then +32.0% on the second attempt (load 12.8). **100 bytes per 500th commit** (about 34 KB/s) failed
  both attempts at +1.39% and +1.75% (load 11).

A least-squares or Theil-Sen slope over the window would not beat the endpoint test on a staircase of
allocator steps. The sound stricter check is to count bytes rather than pages. Add an allocation-balance
test to `undra-ffi`, the only crate allowed a counting allocator (R2), next to `commit_alloc.rs`. Track
live bytes (allocations minus frees, per thread) for the firehose, churn and completion paths. Warm up,
then run 100,000 operations of each, and assert that live bytes return to the warm-up level within a
small constant. That catches a 1-byte-per-operation leak exactly, with no allocator noise and no platform
dependence. Keep RSS for what it is good at: fragmentation and allocator-level growth, over the 60 s
soak.

## The implementer's deviations, judged

1. Two more sustained scenarios (event, fan-out across stores): **accepted**. Cheap, with their own
   invariants.
2. Churn and completions gated at exact bytes: **accepted**. Deterministic for whole 10-op rounds (indices
   are fixed-width `u32`, `crates/undra-wire/src/patch.rs:412-433`). It is a ceiling, though (L7).
3. Churn re-baselined at 169 k: **numbers accepted, explanation rejected** (M6).
4. Permanent fault injection: **accepted, but it was not enough**. Dropping every 101st patch is a
   periodic fault that any end-state check would catch, and it hid H1.
5. One pre-touched drain buffer instead of `Vec<Vec<u8>>`: **accepted**. It is the harness's own queue,
   it does the same copy, RESULTS.md says host stand-ins cost less than platforms do, and peak drains
   (4-8.5 k change-sets × 41 B) fit in the 1.8 MB room.
6. Soak warm-up of half the run: **accepted** on the data above. It is not hiding per-operation leaks;
   the stricter alternative is the allocation-balance test.
7. `--attempts` on the soak: **accepted**. Invariants are not retried (verified with a real breach), and
   the attempt that passed is now reported (L2). In-process retry: L5.
8. Soak completions window 128: **accepted**. It is a carrying-capacity choice for a paced load;
   scenario e still runs 256 in flight.
9. The Kv warning ignored: **accepted**. It is matched on exact target and message, and every other
   warning or error still fails.
10. Debug smoke on 500 churn rows: **accepted**. Release runs 10,000, and the invariants are the same.
11. `DuplicateStress` as its own variant: **accepted**.
12. `Ticker`/`TickSink` write through `update`: **accepted**. It is what makes the allocation gate count
    the commit's own allocations.

## What was checked and passed

* **Timed regions** (question 1). Firehose and event: one `call_sync`/`event`, including the reply drop
  and the host's copy, with the clock read after the operation and `record` outside the timed region.
  Churn: one `churn(1)` call including the host decode and apply. Fan-out: one transaction, plus an `Arc`
  clone for `with_sink` (a few ns against 40 us). Fan-out across stores: likewise. Stream: no per-item
  timing; the fast half's wall time subtracts RSS sampling (`stress.rs:942`). Completions: from `stamp`
  to the reply callback, which includes encoding the call payload (about 100 ns against 200 us).
  Setup (seeding 10,000 rows, attaching 100,000 signals, pre-building payloads) is outside every clock.
* **Histogram**: log-linear, 64 linear buckets and then 32 per octave. The bucket width is 4 ns at 128-255
  ns and 8 ns at 256-511 ns. Nearest-rank percentile, upper bound of the bucket, clamped to the exact max.
  Its unit tests check the 1/32 bound at every value up to 5,000 and geometrically beyond.
* **Invariants, completions and stream** (question 2). Completions: the order is checked on every
  change-set as it is drained. Losses are checked by exact counts at three levels: replies to issued,
  answered to issued, and delivered to walked. The final total must equal the last value the main thread
  applied. `SwapChangeSets` (a one-off swap) is caught and is the only invariant that breaks. Stream
  (scenario d): produced minus delivered is checked after every round at quiescence, which is where any
  buffering beyond credit would show (1.5 M rounds a run here, max ahead 1).
* **Determinism (R12)**: the fixtures use no wall clock, no ambient randomness and no threads. `Churn` uses
  a seeded xorshift64, `Producer` is pure, `Fetcher` goes through a port, and `TickSink` goes through an
  event port. Every `Instant`, thread and sleep is in the harness (`host.rs`, `stress.rs`, `soak.rs`),
  which `bench/src/lib.rs` already places outside R12.
* **RSS fields and units** (question 4): `ps -o rss=` reports in 1024-byte units on macOS, and `VmRSS` in
  kB on Linux. Both are converted to bytes. Sampling runs at 1 Hz in the soak and 4 Hz in scenario d,
  always outside timed regions. Platforms without a source report "skipped", never "passed".
* **Allocation gate** (question 7, `crates/undra-ffi/tests/commit_alloc.rs`). The `// SAFETY:` comments
  are sound. The global allocator forwards to `System` unchanged, and the counter is a `const`-initialised,
  destructor-free `thread_local!` `Cell`. It never allocates, and `try_with` tolerates TLS teardown. The
  count is per thread, and with `core_threads: 0` the commit runs on the test's thread. With a temporary
  print (debug and release): **exactly 3,000 for 1,000 commits after 10 warm-up, 30,000 for 10,000 after
  1,000 more, 3 on the very first commit with no warm-up, and 0 unobserved**. So 3 is the steady-state
  number, not a warm-up artefact. The gate runs in `ci.yml` through `cargo test --workspace`.
* **CI readability**: a failing stress row prints the measurement, the "x" reason under it and a final
  list of every failed gate. The soak prints "FAIL" lines and exits 1, or 2 on a harness error. Both use
  `--nocapture`, so the table reaches the log.

## Runs (all on the fixed or reviewed tree; load average in brackets)

* `cargo test -p undra-bench --test stress --release -- --nocapture` at `7a088a6` [4.4-4.8]: all ok. Firehose
  5.97 M/s, p99 211 ns. Churn 151.8 k/s. Fan-out 11.3 k/s, p99 188 us (the cache-bound row halves under load).
* The same after the fixes [7.7]: all ok, 6 tests including `a_single_dropped_update_fails_the_mirror_invariants`.
* `cargo test -p undra-bench --test budgets --release` after the fixes [10]: ok.
* `cargo run -p undra-bench --release --bin soak -- --seconds 10 --attempts 2`: passed at `7a088a6` [4.5] and
  after the fixes [10-14], the latter at +0.30% RSS with 200,040 patches for 200,040 operations.
  `--seconds 60` at `7a088a6` [4.7-10]: passed, +0.30%, p99 median 40 us and worst 72 us.
* `cargo test -p undra-bench` (debug) [7.4]: all ok, 5.8 s. `cargo fmt --all --check` and
  `cargo clippy --workspace --all-targets -- -D warnings`: clean.
* The experiments (a commit 1.8x slower; 100 B per commit; 100 B per 500 commits; the exact allocation
  counts; churn with a counting host versus an applying host; the empty-step clock cost) used temporary
  edits to `crates/undra-signals/src/txn.rs`, `crates/undra-ffi/tests/commit_alloc.rs` and a scratch test
  file. All of them were reverted before the commit (`git status` clean apart from the fixes).
