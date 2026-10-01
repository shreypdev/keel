# stress-bench (S1a) — decisions (SDE, branch wt/stress)

[VERIFIED by running it, 2026-09-30, Apple M5 Pro / macOS 26.5 / rustc 1.98.1, a shared machine
(load average 3 to 8): the layer A rows and the budgets gate, the sustained gate three times in a
row, the soak at `--seconds 10` (more than twenty runs while tuning, five in a row on the final binary) and at
`--seconds 60` (seven runs, two on the final rules), fmt, clippy, `cargo doc`, the workspace tests
(2,142 passed, 0 failed, 9 ignored; status.md had 2,109). CI has not run: `bench.yml` is edited and YAML-checked, not executed on a GitHub runner.]

This is the Rust half (S1a) of `.10x/specs/2026-09-30-stress-bench-design.md` (integrator decisions
D1 to D6 in `.10x/decisions/architect/stress-bench.md`). Nothing outside `bench/**`, one new test
in `crates/undra-ffi/tests/`, the CI workflow and this note changed: no runtime, no playground, no
site, no wire, ABI or generated shape (so no ADR).

## What was built

| Piece | Where |
|---|---|
| `Histogram` (1,184 counters, 1/32 relative error, no allocation per sample) | `bench/src/stats.rs` |
| `resident_bytes` (`/proc/self/status` or `ps -o rss=`), `RssSeries`, `RssGrowth` | `bench/src/rss.rs` |
| `[stress."name"]` tables, `StressBudget::check` (floors, ceilings, exact bytes, RSS, scale), `MissingGate`/`DuplicateStress` | `bench/src/budget.rs` |
| fixtures `Ticker`, `Churn`, `Producer`, `Fetcher`, `Ticks` + `TickSink` | `bench/common/fixtures.rs` |
| hosts `CopyingHost`, `ApplyingHost` (+`ListMirror`), `DrainHost` (+`Frame`, `MainThread`, `OrderChecker`) | `bench/common/host.rs` |
| 8 layer A workloads and 7 sustained scenarios | `bench/common/stress.rs` |
| criterion bench over `group("stress")` | `bench/benches/stress.rs` |
| the sustained gate, baseline printer, `UNDRA_STRESS_JSON` | `bench/tests/stress.rs` |
| the soak | `bench/src/bin/soak.rs` |
| allocation gate (D4) | `crates/undra-ffi/tests/commit_alloc.rs` |
| budgets, RESULTS.md "Harsh conditions", CI | `bench/budgets.toml`, `bench/RESULTS.md`, `.github/workflows/bench.yml` |

## Numbers (best of three 10 s runs; budgets are derived from these)

| Scenario | Throughput | p99 | p999 | Bytes/op | Gate (floor, p99, p999) |
|---|---|---|---|---|---|
| `firehose/sustained` (call_sync) | 5.93 M/s | 211 ns | 295 ns | 37 | 1.1 M/s, 1.1 us, 3 us |
| `event/sustained` | 7.28 M/s | 167 ns | 251 ns | 37 | 1.4 M/s, 840 ns, 2.6 us |
| `keyed_churn_10k/sustained` | 169 k ops/s | 17.9 us | 26.6 us | 61 | 33 k/s, 90 us, 270 us |
| `fanout/sustained` (1,000 of 100,000) | 23.3 k txn/s | 81.9 us | 129 us | 21,012 | 4.6 k/s, 410 us, 1.3 ms |
| `fanout_stores/sustained` | 8.24 k txn/s | 254 us | 352 us | 33,000 | 1.6 k/s, 1.3 ms, 3.6 ms |
| `stream/backpressure` (fast half) | 28.8 M items/s | - | - | - | 5.7 M/s, RSS 1% |
| `completions/8_threads` | 349 k/s | 737 us | 967 us | 37 | 69 k/s, 3.7 ms, 9.7 ms |

Layer A (p50 per iteration, budget = 5x): `txn_x1000` 78.8 us (400 us), `call_set` 124.7 ns
(630 ns), `event` 115.5 ns (580 ns), `keyed_churn_10k/ops_x1000` 5.85 ms (30 ms), fan-out 100k/1k
37.6 us (190 us), 10k/1k 25.6 us (130 us), 1,000 stores 114.9 us (580 us), `stream/items_x1000`
34.6 us (180 us). Allocations per observed commit: exactly 3.00; unobserved: 0 (`commit_alloc.rs`).

The three runs moved by up to 4x on the cache-bound tails (fan-out p99 82 to 295 us, churn p99 18 to
42 us) because other agents were building; the floors and ceilings are derived from the best run
and passed every other run (the first of three consecutive gate runs needed its second and third
attempt for the churn p999, 491 us and 1.25 ms against 270 us, while the machine was visibly busy:
its throughput halved too). I did not widen anything for that run.

## Deviations from the design, and why

1. **Two more sustained scenarios**: `event/sustained` (g) and `fanout_stores/sustained` (c'). The
   design gave them layer A rows only; the brief asked for the sustained measurements for every
   scenario, and both are cheap and have their own invariants (one 37-byte change-set per event;
   1,000 change-sets and 33,000 bytes per transaction). The table, the gate and the `scenarios()`
   list have seven entries (plus `soak/mixed` in the table).
2. **Churn is gated at 61 bytes exactly, not 1.1x**, and so are completions (37). The design said
   "1.1x measured (random mix)"; with the fixed cycle at fixed widths every run of a whole number of
   rounds ships the same bytes, so the exact value is the honest gate. `bytes_per_op` is a ceiling
   (growth fails; the invariants in code assert the exact 37 for the firehose).
3. **Churn re-baselined**: 169 k ops/s and 5.85 ms per 1,000 ops, against the design's 280 k and
   3.65 ms, which came from a random 20/20/20/40 mix. The fixed cycle has 2 moves and 2 inserts per
   10 (a move is about 9 us on 10,000 rows, `signals/keyed_10k/move`), so it is a harder mix.
4. **`Fault`** in `StressConfig` (not in the design): `SkipPatches` makes the churn host drop every
   101st patch, `SwapChangeSets` makes the main-thread model swap its first two change-sets. They are
   the by-hand check the design asked for ("make `ApplyingHost` skip one patch and see the equality
   invariant fail; make the drain thread swap two change-sets and see out-of-order fail"), made
   permanent: `a_skipped_patch_fails_the_equality_invariant` requires the host-list equality invariant
   to fail, and `swapped_change_sets_fail_the_order_invariant` requires the order invariant to be the
   only one that fails (nothing is lost, every call is answered). `OrderChecker` has its own unit test
   (a repeat, a step back and a swapped pair all fail; interleaved stores do not).
5. **Drain queue is one pre-touched byte buffer (`Frame`), not a `Vec<Vec<u8>>`.** The design said
   `Mutex<Vec<Vec<u8>>>`. With a vector per change-set the soak's RSS crept 0.5 to 5% over 10 s on
   macOS: 170,000 blocks a second allocated on the core thread and freed on the "main" thread, in
   size classes a pre-touch could not predict. One buffer with a length prefix is the same copy,
   the allocator traffic is gone, and the buffer's pages are touched before the run (`vec![0xA5; ..]`:
   zeroes would be demand-zero pages that are not resident). Real platforms do allocate per
   change-set; that cost is theirs to measure on the platform.
6. **Soak warm-up is the first half of the run** (design: 20%). The 60 s runs showed RSS climbing in
   page-sized steps (allocator magazines, thread stacks) until about 30 s and flat afterwards:
   10.44 to 10.67 MB by 10 s, steps at 17 s, 19 s and 33 s, then 10.77 MB to the end; a warm-up of
   20% (12 s) measured those late steps as growth (+0.88% and +2.05%: two of four 60 s runs ended
   over or near the gate). It is a fact about the allocator on this host, not about the core; the
   first half is therefore not counted (both 60 s runs with that rule ended at +0.00%).
   `--warmup PCT` overrides it.
7. **`--attempts N` on the soak** (not in the design): the soak is re-run when only the noisy gates
   (RSS, drift, rate) failed, as the budgets gate takes the best of three; a broken invariant is
   never retried. CI passes `--attempts 2`.
8. **Completions window in the soak is 128, not 64.** At 50,000 per second a window of 64 needs a
   mean call to reply under 1.28 ms; on this loaded host the soak reached 90% of its target (a
   warning), and a slower CI runner would reach under 50% (a failure) for reasons that have nothing
   to do with Undra. 128 reaches 100% here.
9. **The Kv warning is not counted.** `undra-query` logs once that the `Kv` port never became
   available; nothing in the harness binds a `Kv`, so `CountingHost` ignores exactly that record and
   every other warning or error still fails the run. `DrainHost` answers only `SOURCE_PORT` later
   and every other port "unavailable" (it first answered all of them with `1u64`, which made the
   query client's hydration count as two extra completions).
10. **Debug smoke uses 500 churn rows**, not 10,000 (`CHURN_ROWS`): an unoptimised op on 10,000
    rows takes 2.8 ms, which made `cargo test -p undra-bench` 11 s slower. The invariants are the
    same; release runs the real size. Debug `cargo test -p undra-bench` takes 5.5 s in all (budgets smoke 0.9 s,
    stress smoke and the two fault tests 2.8 s).
11. **`DuplicateStress`** is a separate `BudgetError` variant rather than reusing `Duplicate`, so
    the existing `Duplicate { name }` test and its "[bench.\"..\"]" message keep meaning what they
    say.
12. **Ticker/TickSink write through `update`**, not `set` (a `Signal::set` boxes its new value), so
    the allocations the ffi gate counts are the commit's alone, which is the claim D4 gates.

## What I would watch

* The RSS gate on macOS sits near the allocator's noise floor (1% of an 11 MB process is 110 KB,
  about seven pages). With the half-run warm-up the two 60 s runs ended at +0.00% and the five
  10 s CI-configuration runs at +0.00% to +0.72%; the six before them (same rule) had one at +1.44%.
  On Linux's per-thread arenas I expect it flat, but CI has not run. If it flakes there, raise the
  limit in the workflow with `--rss-limit-pct`, not in the file.
* The rate gate ("a second under half its target fails") is the one noisy gate that tripped on its
  own: one of the last three 60 s runs on the final tree failed on a 47% completions second while
  another build ran (load average 7 to 9); the other two passed, and a rerun with `--attempts 2`
  passed on its first attempt. The load is paced and carried at 100% of target in every quiet run;
  a runner that cannot carry it is a runner the gate should say so about.
* The firehose p99 inside the soak is 40 to 80 us, not 211 ns: it is the wait for the core lock
  behind a 20-operation churn call or a completion burst, which is what mixed load looks like. The
  drift gate compares second with second, so the number itself is not gated.
* The completions gate couples throughput and latency (a closed loop of 256 calls: mean latency is
  the window over the throughput), so a runner too slow for the floor also breaks the p99 ceiling;
  `UNDRA_BENCH_SCALE` moves both.
* Everything here is the core side. The web numbers, the device numbers and the ADR-031 before and
  after belong to S1b and the device phase and are not in these tables.

## Crossing the rename

`main` had merged the rename to Undra (ADR-030). `git merge main` conflicted in the files this branch
owns (`bench/budgets.toml`, `bench/common/fixtures.rs`, `bench/common/host.rs`,
`.github/workflows/bench.yml`; kept this side) and in the sde index (took main's, re-added this
note's line); `commit_alloc.rs` landed in `crates/undra-ffi/tests/` with the directory. Then
`scripts/rename-keel-to-undra.sh bench crates/undra-ffi/tests .github/workflows/bench.yml
.10x/specs/2026-09-30-stress-bench-design.md .10x/adrs/ADR-031-frame-coalesced-delivery.md
.10x/decisions` (21 files; it also rewrote the three `launch-v2.md` records of other roles because
naming `.10x/decisions` lifts their exclusion, so those were reverted); it rewrote this note's names
too. After it: fmt, clippy
`--workspace --all-targets -D warnings`, `cargo doc`, the workspace tests (2,143 passed, 0 failed,
10 ignored), the budgets gate (`undra-bench`), the stress gate, the 10 s soak and two 60 s soaks
ran on the renamed tree; env vars are now `UNDRA_STRESS_SECONDS`, `UNDRA_BENCH_SCALE`,
`UNDRA_BENCH_FILTER`, `UNDRA_BENCH_BUDGETS`, `UNDRA_STRESS_JSON`.

## Follow-ups closed (2026-09-30, `wt/bench-followups`)

[VERIFIED by running it, 2026-09-30, Apple M5 Pro / macOS 26.5 / rustc 1.98.1, a shared machine (load average
2 to 5 for the runs below): `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, the
workspace tests (2,209 passed, 0 failed, 10 ignored), the budgets gate in release with and without
`UNDRA_BENCH_BASELINE=apple-m5-pro`, the stress gate three times in a row (every scenario passed on attempt 1), the
10 s soak (five runs with the drift gate) and the 60 s soak (nine runs of the final code on the unmodified core, and
three with a commit made to slow down on purpose). `scripts/bench-record-base.sh` ran end to end on the host (that is the CI path); `bench.yml` was
YAML-checked and **has not run on a GitHub runner**.]

Closes the open findings of `.10x/reviews/2026-09-30-stress-bench-review.md`: M1, M3, M4 and L4 to L8 (M2 is the CI
running; I used its six logs, below). Nothing outside `bench/**`, `scripts/bench-record-base.sh` and
`.github/workflows/bench.yml` changed (and no runtime, wire, ABI or generated shape, so no ADR). Commits, oldest first:
`0aa4412` (a bug found on the way), `bf9bbcf` (M1), `aee2b1d` (M3), `449e196` (M4), `3076cc5` (L4 to L8), `8004a21`
(result files of failing runs), `acc4a22` (the host baseline, re-recorded) and the commit of this section (the
numbers, `bench/results/`, RESULTS.md).

### What was built, and the evidence that it works

| Finding | What | Evidence |
|---|---|---|
| M1 | Five `[ratio."..."]` gates (two layer A rows of one run, so the machine's speed cancels); `UNDRA_BENCH_BASELINE` / `UNDRA_BENCH_RECORD` baselines (`bench/baselines/apple-m5-pro.toml`: p50 1.5x, 20 ns of slack, throughput 1/1.5, p99 2.5x); on CI the baseline is the **merge base measured in the same job on the same VM** (`scripts/bench-record-base.sh`, wired in `bench.yml`) | the review's experiment, repeated (a spin in `commit`, +71 ns, never committed): `txn_x1000` 77.5 -> 148.5 us, every absolute gate passes (margins 2.7x to 2.8x on the three rows, 4.0x or more on the rest, sustained firehose 4.01 M/s against a floor of 1.1 M/s). Then `commit_vs_bare_call` 1.74 -> **3.37** (max 2.3) and `set_vs_bare_call` 2.82 -> **5.08** (max 3.6) fail; against the base recorded two minutes earlier on the same machine the rows read 0.99x/0.98x/1.01x unchanged and **1.90x/1.75x/1.77x** slowed, and both firehose floors fail; against the committed host baseline 1.61x/1.70x/1.72x (a weaker spin that run). Files: `bench/results/2026-09-30-commit-{control,spin,spin-vs-host-baseline}-*.json` |
| M3 | `completions/contended`: a host thread `call_sync`s `Fetcher::bump` into the store the 8-thread completions write, as fast as it gets the core lock; the store's total is one signal every write adds one to and the main thread checks it arrives as `0, 1, 2, ..` (`track_sequence`), in e as well | 345 k writes/s (302 k completions + 43 k host writes), 0 lost, 0 out of order, 0 steps that were not +1, exact total; `Fault::SwapChangeSets` fails order and exact count only, the new `Fault::DropChangeSet` fails walked count and exact count only, both scenarios |
| M4 | `undra_bench::stats::drift`: a Theil-Sen line through the per-second p99s of the second half must not rise by more than 50% of their median (at least 6 seconds), next to the spike gate | unit tests (a steady climb 40 -> 100 us and a steady doubling fail, the doubling passes the old gate; flat, mild, falling and two-bad-seconds series pass; a real soak's second half passes at -5%); the real soak, 8 of 9 60 s runs pass; a commit that slows steadily during the run passes every other gate and fails this one (+95%, `commit-ramp-soak-60s.json`) |
| L4 | a warm-up before every measured run (a tenth, at most 200 ms, `UNDRA_STRESS_WARMUP_MS`), reported in each result | `a_warm_up_runs_first_and_is_not_measured` over six scenarios |
| L5 | `--attempts N` re-executes the soak per attempt; exit status 0/1/2/3 | a 30 s soak with a steadily slowing commit and `--attempts 2`: the parent re-executes the binary, both attempts run to a verdict and fail the trend (+90%, +82%), exit status 1 |
| L6 | `rss_caveat()` printed by the stress test, the soak (top and RSS verdict) and RESULTS.md Method | the soak and stress output |
| L7 | `bytes_per_op` documented as a ceiling (budget.rs, budgets.toml, RESULTS.md); churn asserts exactly 61 bytes per operation, completions 37 per change-set | the debug smoke run (500 rows) and the release runs |
| L8 | `UNDRA_BENCH_RESULTS_DIR` / `UNDRA_BENCH_RESULTS_TAG`: one JSON per scenario, layer A and soak with command, machine, load; failing runs say so and why; `bench.yml` uploads them | `bench/results/` (39 files); every number of RESULTS.md's tables is one of them |

### Judgement calls (where I did not do what the review proposed, or chose)

1. **CI baseline = the merge base on the same VM, not a runner-recorded `bench/baselines/<env>.toml`.** The review
   proposed recording the runner's own baseline after the first green runs. Six runs of the workflow (their logs,
   `gh run view`) show the ubuntu-latest pool is at least two machine classes: `stress/firehose/txn_x1000` 123 to
   275 us, `signals/changeset_100/cell` 1.56 to 4.44 us across runs, in two clusters. A file recorded on one class
   fails 1.5x on the other. A baseline of a base measured in the same job cancels that by construction.
2. **The ratio gates are limited by the spread of the ratio across that pool, and I say so.** Same-family ratios spread
   1.2x to 1.5x, cross-family 2x to 3x (the runner runs a memmove-bound list operation 1.2x slower than the host and an
   atomics-bound call 3x slower). Only `commit_vs_bare_call` and `set_vs_bare_call` can see a 2x commit; modelled on the
   recorded samples, a 2x commit fails the first on all eight and 1.8x on seven (the fastest runner run passes at 2.26
   against 2.3). The `max` of each is 1.15x the largest healthy ratio, rounded up, not fitted to pass the experiment.
   I did **not** add the review's "completions vs firehose" ratio: a closed loop of 256 calls answered by threads and
   a single-thread rate are different families, and the host's own two completions regimes (below) move that ratio 1.3x
   by themselves. "Churn per op vs firehose per op" is `keyed_churn_vs_set`, wide on purpose.
3. **Theil-Sen, not least squares, for the trend.** Still a linear regression (the review and the brief say so), but
   one bad second at the end of the window tilts least squares (a flat 40 us series with a 400 us last second rises 40%
   of its median under least squares and 0 under Theil-Sen), and that second is the spike gate's. The limit is the
   host's: its scheduler flips the firehose between a ~40 us and a ~110 us p99 regime (at 7, 11, 15 and 27 s in one
   run), and a step that stays reads as a climb (a unit test says so). One of nine 60 s runs failed at +52% against
   +50% while another build ran, so the gate has a false-positive rate on a shared host (about one in nine at 60 s, none
   in six 10 s runs), absorbed by `--attempts 2`; I did not widen the bound, because at 0.67 a steady doubling (rise
   0.67) would pass, which is the case the review asked it to fail.
4. **The warm-up is a phase before the measured run**, not the first samples discarded: the measured duration is
   unchanged and the counters are read after it. The invariants cover both. For the completions scenarios `ops` is
   the change-sets delivered since the warm-up ended (each exactly 37 bytes, asserted), which keeps `bytes_per_op`
   exact where two counters read at one instant would be off by one.
5. **The soak's JSON is now one object** (command, machine, load, `run`, `summary`, `windows`), not an array of rows,
   and its exit status has a 3 for a broken invariant. Nothing consumed either.
6. **Scenario e' serialises its two committers through the core lock** (`Runtime::call_sync` and a task poll both take
   it), so it proves the hand-off and the delivery under it, not concurrent commits to one store; the writer is
   unthrottled (it yields) and measures about 43,000 writes a second beside 302,000 completions. RESULTS.md says this.

### Found on the way

* **The completions scenario ignored its deadline** (`0aa4412`): the issuer read the clock only when its window of 256
  filled, and when the completers kept pace it never did, so a 200 ms debug run took 9 s, 43 s and 154 s under load (the
  debug smoke of `cargo test --workspace` was a lottery between 1 s and 90 s). Now every 32 calls; four 100 ms runs
  must each end in 5 s (the old loop did not return in 170 s, twice); the whole debug stress suite takes 1.9 s.
* **A base build in the head's target directory silently makes the head measure the base against itself**: cargo keys a
  workspace member's artifacts without its path, so the base build overwrote the head's test binaries (the budgets file
  the "head" binary read was the deleted worktree's) and the head's `cargo test` found them fresh. The script now uses a
  target directory of its own and refuses a shared one. This would have made the CI baseline gate a no-op.
* The commit-path spin is not 1.8x on every run: 135 iterations cost +63 to +71 ns on this host depending on frequency
  state (txn_x1000 1.6x to 1.9x), so the experiment files read 1.6x to 1.9x, not exactly 1.8x. The M1 commit message
  quotes the first runs.
* The completions scenarios run in one of two regimes per process on this host (call to reply p50 ~170 us at ~340 k/s,
  or ~560 us at ~260 k/s); the first baseline recording landed in the slow one. The baseline keeps the best of three.

### What I would watch

* The first run of `bench.yml` on a runner: `fetch-depth: 2` and `HEAD^1` for a pull request's merge ref, `git fetch
  --depth=1 origin <sha>` for a push, `target/bench-base` not being in the cache (a cold dependency build, about a
  minute), and the job going from about 2.5 to about 5 minutes. If the base does not build against the head's harness the
  step says so and the gates are the budgets and the ratios.
* Noise at 1.5x: `signals/keyed_100/insert` (333 ns) read 1.43x its baseline in a run where nothing touched it, and
  passed because the best of three counts. A row that proves noisy on a runner gets its own `tolerance`.
* The ratio `max` values come from six runs on (at least) two hardware classes; a third class can break
  `commit_vs_bare_call` (max 2.3). The `bench-results` artifact of every run is the data to re-derive them from.
* The sustained gates see a commit regression of about 1.6x and up (the call, the clock and the host's copy dilute it);
  the layer A rows are the sharper instrument. The baseline stale-row test fails a renamed workload (it would silently
  leave the gate); a new workload is only a notice until the baseline is recorded again.
* Still not built from the review: the allocation-balance test in `undra-ffi` (the exact memory check; the macOS RSS
  caveat stands until it exists) and the optional per-second stream check (L3).
