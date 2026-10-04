# Benchmark follow-ups (M1, M3, M4, L4-L8) - adversarial review

**Date:** 2026-10-01 · **Reviewer:** Claude Opus 5.5 (adversarial pass: does CI now gate performance,
and only performance) · **Piece:** `wt/bench-followups` at `0197074`, review fixes committed on top ·
**Closes:** the open findings of `.10x/reviews/2026-09-30-stress-bench-review.md` · **Implementer's
record:** `.10x/decisions/sde/stress-bench.md`, "Follow-ups closed"

Machine: Apple M5 Pro, 18 cores, macOS 26.5, rustc 1.98.1, **shared** (load average 1.3 to 3.6 during
these runs). CI evidence: the logs of **twelve** Bench runs on `shreypdev/undra` (`gh run view <id> --log`):
the implementer's six (36798191863 to 36800063172) and the six that came after them (36800899301,
36803857331, 36804070521, 36806665630, 36808385622, 36808470795), which are an out-of-sample check of
every number set from the first six.

## Verdict

CI now fails a 2x regression of the commit path on every runner seen: `commit_vs_bare_call` fails on all
twelve runner samples at 2x (and on both host samples), and the base-commit baseline catches 1.8x on the
three fast-class runner runs where the ratio does not (`txn_x1000` alone reads 1.8x its base, over 1.5x).
As delivered, though, two things would have undone that in practice. The fan-out ratio gate fails a
healthy commit on a runner its six samples did not include (2.37 against 2.3 on a docs-only commit,
run 36806665630). And the workflow's `cancel-in-progress` on main cancelled exactly the runs that matter
here: a merge followed a minute later by its state commit (36803857331, 36808385622), so the baseline gate
would have compared the state commit with the merge and never the merge with what it replaced. Both are
fixed here (High), along with a silent no-baseline path and a remedy for a noisy row that could not exist
on CI (Medium). **The first run after merge will be green and is an A/A run by construction**: simulated
end to end on the host (the merge of this branch with `main` at `74106e1`, the base at `74106e1`), the base
records 55 rows and 8 scenarios in 112 s with the head's harness (the base never needs `UNDRA_BENCH_RECORD`
of its own: the script copies the head's `bench/` into it), and the head passes every gate against it,
worst layer A row 1.13x its base. Watch that first run's "vs base" column: it is the only same-VM noise
measurement the runner has given so far.

## Findings

| # | Sev | Where | Finding | Status |
|---|---|---|---|---|
| H1 | High | `bench/budgets.toml` `[ratio."fanout_100k_vs_10k_observed"]` (max 2.3) | **A gate that fails for reasons unrelated to the code.** The max was 1.15x the largest of six runner samples (1.92). Of the six runs after them, 36806665630 (a docs-only commit on `main`) reads **2.366**: CI would have gone red. The ratio is cache-bound and spreads 0.99 to 2.37 across the twelve runner runs (2.4x), so the 1.15x rule cannot bound it. | **Fixed**: max 4, the bound of what it guards (the layer B scenario's own 4x; commit cost following the observed count would read about 10, so 4 still sees it on any runner). `SAMPLES` in `tests/budgets.rs` now holds all twelve runner runs plus the two host runs; `budgets.toml` and RESULTS.md say why this one ratio departs from the rule. |
| H2 | High | `.github/workflows/bench.yml:51-53` (`concurrency: bench-${{ github.ref }}`, `cancel-in-progress: true`) | **The baseline gate silently a no-op for merges.** For a push the base is `github.event.before`. When a push's run is cancelled, nothing ever compares that push with what it replaced, and the next push is gated against it, regression included. This repo's workflow pushes a merge and then its `state(...)` commit a minute later; 3 of the last 13 Bench runs were cancelled, two of them merges (`wt/coalesce`, which changed the runtime, and `wt/stress-screen`). With `cancel-in-progress: false` a third queued run still cancels the pending one, so only a group per commit closes it. | **Fixed**: `group: bench-${{ github.event_name == 'pull_request' && github.ref \|\| github.sha }}`: a pull request's newer run still replaces its older one (each compares the whole PR with its base); a push to main is never cancelled. |
| M1 | Medium | `.github/workflows/bench.yml` base step | **The no-baseline path was silent.** A base that fails to build (or to fetch) failed a `continue-on-error` step; the job stayed green and the only trace was "no baseline selected" in the Budgets log. A permanently broken base step (a fetch that never works, say) would have switched the gate off for good with nobody told. | **Fixed**: `::warning title=No baseline gate::...` on the run for both "no base" and "base could not be measured", which says what still gates. Simulated: a base from before the rename (`815c822`) fails in seconds with `bench-record-base.sh: nothing was recorded (the base did not build against this tree's bench/?)`, exit 2, worktree cleaned. |
| M2 | Medium | `bench/RESULTS.md` ("a row that proves too noisy there gets its own `tolerance` in the baseline file"), `bench/src/baseline.rs` | **The documented remedy for a noisy row does not exist on CI**: that baseline is recorded afresh in every job, so it can never carry a row factor; the only knob was `UNDRA_BENCH_BASELINE_TOLERANCE`, which widens every row. | **Fixed**: `baseline_tolerance` (at least 1) in a `[bench."..."]` or `[stress."..."]` table of `budgets.toml` gives that row **at least** that factor against any baseline (`Selected::bench_gate_with`, `stress_failures_with`; a baseline file's own row `tolerance` still wins). Parser and gate unit-tested. Set on the cache-bound rows only: the three layer A fan-out rows at 2 (88 to 132 us across runner VMs of one class, against 1.3x for the commit rows) and the two fan-out scenarios at 3 (their p99 moved 82 to 295 us between runs on a busy host). The commit rows that carry a 1.8x regression keep 1.5x. |
| L1 | Low | `bench/src/{baseline,lib}.rs`, `bench/budgets.toml` | Three references to `scripts/bench-vs-base.sh`, which does not exist (`bench-record-base.sh`), and "merge base" where the base is the PR's target tip or the commit a push replaced. | **Fixed.** |
| L2 | Low | `scripts/bench-record-base.sh:58-61` | The shared-target refusal compared `cd`-resolved paths: a directory that did not exist yet resolved to nothing and passed (`BENCH_BASE_TARGET=$ROOT/target` on a fresh clone would have rebuilt the very bug). A relative `BENCH_BASE_TARGET` was handed to cargo after `cd "$WORK/tree"`, so it landed inside the throwaway worktree and was deleted every run. | **Fixed**: both directories are created, compared as physical paths (`pwd -P`), and the base's is passed to cargo absolute. |
| L3 | Low | `bench/baselines/apple-m5-pro.toml` `[meta]`, `record()` in both tests | Provenance lacked the command (three `UNDRA_BENCH_RECORD_BEST=1` recordings, only the last one's load kept). | **Fixed**: recordings write `command_layer_a` / `command_layer_b`; the committed file carries both, from the commit message of `acc4a22`. |
| L4 | Low | `bench/common/stress.rs` (`completions_contended` docs) | Said two threads "commit to one store at once"; the core lock serialises them (`serve_sync` and a task poll both enter it, `runtime.rs:793`, and `write_allowed` refuses a write without it). RESULTS.md and the decision record already say "the hand-off". | **Fixed** (wording). |
| L5 | Low | `bench/tests/stress.rs` `the_completions_scenario_stops_near_its_deadline` | The regression test is real, but under the old loop it **hangs** rather than fails: with `0aa4412` reverted in a scratch tree the debug test was still running after 150 s (killed); fixed, it takes 0.5 s. A hang fails only at the CI job timeout. | **Fixed**: the scenario runs on a thread behind a 60 s `recv_timeout`. |
| L6 | Low | `bench/RESULTS.md` "The CI gate" | Called the bytes gate "exact change-set bytes" (it is a ceiling; the exact count is each scenario's invariant, L7 of the earlier review). | **Fixed** (wording). |
| L7 | Low | `.github/workflows/bench.yml` (`Swatinem/rust-cache@v2`) | rust-cache saves only on a key miss, and the current key is an exact hit ("Cache up-to-date" in every recent log), so `target/bench-base` would never be cached and the base's dependencies would build cold on every run until `Cargo.lock` or the toolchain changes (about a minute on the runner). | **Fixed**: `key: with-base-target` forces one save that includes it (rust-cache treats a directory with `CACHEDIR.TAG` as a nested target and keeps its dependencies). Check the first run's post-job log: the cache size should roughly double from 83 MB. |
| L8 | Low | `bench/tests/budgets.rs` (1.8x test), RESULTS.md | "1.8x fails on seven of eight, only the fastest runner passes": on the twelve runner runs, **three** fast-class runs pass at 1.8x (2.23 to 2.28 against 2.3; 36806665630 fails at 2.302). | **Fixed**: the test asserts the three, the text says the baseline is the gate there. 2x still fails all fourteen samples. |

High/Medium: 2 High and 2 Medium, all fixed. Low fixes made directly: 8.

## The questions, answered

### 1. The CI baseline mechanism

**(a) Which base.** Pull request: `HEAD^1` of the merge ref, the target branch's tip when GitHub computed
the merge (`fetch-depth: 2` has it). Push to main: `github.event.before`, the tip the push replaced; with
several commits in one push it is older than `HEAD^1` and the script fetches it (`git fetch --depth=1 origin
<sha>`, which GitHub allows for a reachable commit). A branch creation (`before` all zeros) has no base and
now warns. Both are what a gate wants: the whole change against what it changes. The hole was cancellation,
not the choice (H2).

**(b) The first run after merge.** The base predates `UNDRA_BENCH_RECORD`, and that does not matter: the
script deletes the base's `bench/` and copies the head's in, so the base runs the head's harness against its
own crates. This piece changes no crate, so that first run is an A/A run. Simulated exactly (merge of
`0197074` with `74106e1` in a scratch worktree, base `74106e1`, `UNDRA_BENCH_SCALE=2`, fresh target
directories): **exit 0**, 55 layer A rows and 8 scenarios recorded (`git = "74106e12f808-dirty"`, which is
right: the base tree carries the head's `bench/`), then budgets, stress and a 10 s soak with `--attempts 2`
all passed against it; worst layer A row 1.13x its base (`wire/map100_u32_u32/roundtrip`;
`signals/observe_100_initial` and `snapshot/restore_100kb` 1.12x), every ratio and every scenario inside. The other path, a base that cannot
build the head's harness (`815c822`, before the rename), fails fast with a clear message and exit 2; in CI it
now warns and gates on the absolute budgets and ratios. It neither fails the job nor passes silently. The
literal merge base `b993067` is the same A/A case (no crate differs from the head).

**(c) Shared target.** Confirmed fixed: the base builds in `target/bench-base` (or `BENCH_BASE_TARGET`), the
script refuses the head's own directory (L2 made that refusal hold for directories not yet created and for
symlinks). Cargo's shared state cannot leak artifacts across: `~/.cargo/registry` and `git` hold sources
only, content-addressed by version, and every compiled artifact and fingerprint lives under the target
directory. The base's `Cargo.lock` edits, if any, happen in the throwaway tree. `UNDRA_BENCH_RESULTS_DIR` is
unset for the base, so its files never mix with the head's artifact.

**(d) Time.** The current job is 2 min 2 s to 2 min 40 s: the head's release build 59 s (workspace crates
only; dependencies cached), budgets 8 s, the stress build 19 s and run 18 s, the soak build 22 s and run
10 s. On the host the base recording took 112 s, builds included (32 s from an empty target directory on 18
cores), against the head's 29 s for running the same two tests (budgets 23 s at three attempts on every row
against 8 s; stress 56 s at three attempts on every scenario against 21 s). Scaled to the runner: base dependencies about 1 min cold (cached after L7), base crates
about 1 min, base stress build 20 s, base budgets about 25 s, base stress about 55 s, plus 3 s for the new
contended scenario on the head: **+3 to 4 minutes, a job of about 5.5 to 6.5 minutes**, well inside its
30-minute timeout. Caching the base's dependencies is L7; caching its workspace crates by base sha is not
worth it (they change on every push to main).

**(e) Noise.** Best of three on both sides: the base always runs three attempts per row (and keeps the best
of each sustained metric over three); the head stops at the first attempt under its gate, so when it fails
it has had three too. The six logs the implementer read measure **different VMs** (rows 1.1x to 3.3x apart):
that spread is what the same-VM design removes, so it says nothing about a 1.5x same-VM gate. What does: the
CI soak's firehose p99 per second over 10 s is flat to within 3% on all ten logged soaks (Theil-Sen rise -4%
to +3%), the host A/A above (worst 1.13x), and the implementer's control (0.98x to 1.03x on the commit
rows; tiny rows to 1.4x, inside the 20 ns floor). A frequency-state change between the base and the head is
a uniform slowdown; it would show as every row moving together, which at under 1.5x passes and above it is
indistinguishable from a regression by any gate. The rows at real risk are the cache-bound ones (fan-out:
88 to 132 us between VMs of one class) and the completions p99 if the base and head processes land in
different scheduler regimes (on the host: p50 about 170 us or about 560 us per process; seven host runs
here all landed in the fast one, p99 0.95x to 1.45x its base). **Safe setting**: keep 1.5x for p50 and
throughput and 2.5x for p99 globally, with the 20 ns floor; the cache-bound fan-out rows 2x and the fan-out
scenarios 3x (M2, set); if the first CI runs show the completions p99 over 2x its base, give
`completions/*` `baseline_tolerance = 3`. Never raise the global factor: at 1.6 or more the gate stops seeing
the 1.8x commit on `txn_x1000` with any margin.

### 2. Ratio gates

Six runner samples are not enough to set a 1.15x bound in general: the chance that the next sample exceeds the
largest of n is 1/(n+1), 1 in 7 at six, and 1.15x only covers that excess when the ratio's spread is well
under 15% within a machine class. Re-derived from the twelve logs (`bench/tests/budgets.rs` `SAMPLES`):
`commit_vs_bare_call` 1.24 to 1.94 (the slow class 1.82, 1.82, 1.94: max 2.3 is about 6 standard deviations
above that class), `set_vs_bare_call` 2.25 to 3.01 (about 3.5), `event_vs_set` 0.76 to 0.89 (about 6),
`keyed_churn_vs_set` 15.1 to 27.3 against 54. None of those four exceeded its in-sample maximum in the six
later runs; their false-positive rate is dominated by a runner class not yet seen, not by noise, and each
failing ratio is re-measured up to three times. `fanout_100k_vs_10k_observed` exceeded its gate out of
sample (H1): about one run in twelve, now fixed. Arithmetic checked from both sources: run 36799240121,
122.8 us / 1000 / 97.8 ns = 1.256, x1.8 = 2.26 (the implementer's number); `commit-spin-layer-a.json`
3.371 after three attempts (the first pass's rows give 3.337; the retry re-measured both rows), control
1.740 from its rows exactly.

**The slowest runner and the fastest.** The slow class (txn 253 to 276 us) has the *highest* healthy
commit ratio, so a 2x commit fails it by the widest margin (3.64 to 3.88 against 2.3), and 1.8x too (3.28
to 3.49). The weak spot is the fast class: at 1.8x three of its runs pass (2.23 to 2.28). That is
acceptable only because the base gate covers it, which is why H2 (the base gate silently off for merges)
was High.

### 3. M3, contended completions

The writer's `call_sync` and the core thread's task poll both take the core lock (`enter_core`,
`runtime.rs:793`; `serve_sync` "runs a synchronous call under the core lock"), and `write_allowed` refuses a
signal write from a thread that does not hold it. So the two committers alternate: the scenario proves the
hand-off and the delivery under it, as RESULTS.md and the decision record say (the scenario's doc comment
did not: L4). The exact-total check is strong: the store's total is one signal every completion and every
write adds one to, the main thread starts from the `observe` value (0) and counts any value that is not the
previous plus one, so a lost, duplicated or reordered change-set breaks it at once; a change-set lost before
the first walk is caught by the walked count. `SwapChangeSets` fails exactly "out of order" and "exact
count", `DropChangeSet` exactly "saw every change-set" and "exact count", in both scenarios (`assert_eq!(len,
2)`), both passing here. The scenario holds on `main`'s crates after ADR-031 (the A/A run).

### 4. M4, the drift gate

The implementation is Theil-Sen as described: every pairwise slope, their median, the intercept the median
of the residuals; the rise is slope x span / median. The tests do what they claim (steady climb and doubling
fail, flat, mild and falling pass, two bad seconds at the end do not tilt it). On CI (10 s, warm-up 50%) the
window is seconds 5 to 10, **exactly six points, so the trend gate is engaged** (`DRIFT_MIN_WINDOWS = 6`);
on the ten CI soaks it would have read -4% to +3% against +50%, so no false positives there. What it can
see in a 5 s span is a climb of 50% within five seconds; a slow drift is the 60 s local soak's job, where
the one-in-nine false positive on this shared host becomes about one in eighty-one with `--attempts 2`
(fresh processes, so the attempts are independent): acceptable locally. CI's soak gates: RSS (+1% or 64 KiB
over seconds 5 to 10), the spike (worst second 3x the median), the trend, every rate at half its target, and
the invariants.

### 5. The completions deadline fix (`0aa4412`)

Confirmed: the clock is read every 32 calls and on every full window. One `Instant::now` per 32 calls of
about 1 us each is under 0.1%, the full-window path yields and reads the clock exactly as before, the latency
is stamp to reply, and `elapsed` and `ops` both include the drain of the calls in flight, so nothing is
biased. The regression test is real: reverted in a scratch tree, the debug test had not returned after 150 s;
fixed, it takes 0.5 s (L5 makes the old behaviour fail in 60 s instead of hanging).

### 6. L4 to L8

* L4: every scenario warms up before its measured run (a tenth, at most 200 ms), each result file records
  `warmup_ms`, and the churn and completions invariants count the warm-up's operations too.
* L5: `--attempts N` re-executes the binary per attempt; exit 0 passed, 1 a noisy gate, 2 harness or usage,
  3 an invariant; the parent retries only 1 and returns 3 at once; a crash or signal is 2.
* L6: `rss_caveat()` is printed by the stress test, the soak (top and RSS verdict) and stated in Method.
* L7: `bytes_per_op` is called a ceiling in budget.rs, budgets.toml and RESULTS.md (L6 above fixed the last
  "exact"); churn asserts exactly 61 bytes per operation, completions 37 per change-set.
* L8: spot-checked against the files: firehose 6.25 M/s, p50 125 ns, p99 211 ns, p999 295 ns
  (`2026-09-30-firehose-sustained.json`); e' 345 k writes/s, p99 336 us, the writer's p50 1.2 us and max
  0.49 ms (`2026-09-30-completions-contended.json`); the 60 s soak +0.14% RSS, trend -26%, p99 median 96 us,
  worst 121 us, largest drain 6,633 (`2026-09-30-soak-60s.json`). All match RESULTS.md, all from
  `acc4a22` (not dirty), with the command and load in the file.

### 7. Hygiene

`bench/results/` is 39 files, **220 KB**: the evidence behind published numbers, small, fine in git. No
`unsafe` added (`bench/src/lib.rs` and `soak.rs` keep `forbid(unsafe_code)` and `deny(missing_docs)`), no
dependency added (`bench/Cargo.toml` unchanged), every new `pub` item documented. The baseline file records
CPU, cores, OS, rustc, date, commit and load, and now the commands (L3).

## Runs (all on the reviewed tree unless stated; load average 1.3 to 3.6)

* The first-run simulation above: base recording 112 s, exit 0; head budgets (6 passed), stress (11 passed),
  soak 10 s `--attempts 2` passed, all against it.
* A base that cannot build the harness (`815c822`): exit 2 in seconds, the message above, no worktree left.
* Completions and contended, six fresh processes against the recorded base: all in the fast regime, p99
  0.95x to 1.45x the base, throughput 0.97x to 1.0x.
* The old completions loop in a scratch tree: the deadline test still running at 150 s; the fix 0.5 s.
* `scripts/bench-record-base.sh b993067` from this worktree (the literal merge base; with the fixes): exit 0
  in 108 s, 55 rows and 8 scenarios; the head's budgets gate against it passed, worst row 1.16x
  (`wire/duration/roundtrip`, 15 ns, inside the 20 ns floor).
* After the fixes (load 2.1 to 2.6): `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --
  -D warnings` clean; `cargo test --workspace` **2,211 passed, 0 failed, 10 ignored** (two more than before:
  the row-factor gate and parser tests); `cargo test -p undra-bench --test budgets --release` 6 passed (the
  fan-out ratio at 1.26 against 4), and again with `UNDRA_BENCH_BASELINE=apple-m5-pro` 6 passed;
  `cargo test -p undra-bench --test stress --release` 11 passed, every scenario on attempt 1, and again
  against `apple-m5-pro` 11 passed; `cargo run -p undra-bench --release --bin soak -- --seconds 10 --attempts
  2` passed on attempt 1 (RSS +0.15%, trend +10% over 6 seconds); `bash -n scripts/bench-record-base.sh`
  clean; `bench.yml` parses (Ruby YAML).
* Scratch worktrees, target directories and the temporary revert of `0aa4412` lived under the session's
  scratch directory and were removed; nothing outside this worktree changed.
