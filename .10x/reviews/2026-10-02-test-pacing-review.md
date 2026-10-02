# Review: test-pacing (wt/test-pacing), 2026-10-02. Interim: the session ended before the full matrix and CI

Reviewer: the adversarial reviewer of the piece. The implementer's head was `5ad3184`. This record is the state when the
session closed. It is not a merge verdict.

## Verdict so far

The piece is sound in intent, but on its own it was **not ready**. One test it called machine-independent still failed on
a stalled machine. Three bounds were weaker than on the sibling platforms. A burst test passed a mutant four times in five.
The `--slow` pass had dropped most of the Rust tests that read the clock. All of these are fixed on the branch.

The contract-grid work the coordinator added (S30 and the grid's small bounds) is committed but only partly run. It **must
not merge until the full matrix and CI are green** (see "What remains").

## Findings

| # | Sev | Finding | State |
|---|---|---|---|
| H1 | High | TS db-suite, "outer statement Busy": the guard's state and the elapsed time were read after the five transaction statements. A slow machine failed the check with Busy on time; Busy at once passed it when they were slow. Shown: 250 ms between the statements fails the old check. | fixed f2d4488 (read as Busy settles; the guard is armed after the statement's timer, so 1.5x like Swift) |
| M1 | Med | Swift DbReview kept an absolute `slowest < 200 ms`, the class removed from its TS sibling. | fixed 8f0d9d4 (an order test with a 60 s busy timeout) |
| M2 | Med | Kotlin burst case: "one whole in 40" passed `QUIET_MILLIS = 1` (8 of 200 whole). The real binding gives 297 to 300 of 300 whole. | fixed e8b33be (most of up to 10 bursts must be whole) |
| M3 | Med | Kotlin and Swift lone-message tests had no lower bound. The Swift reference used `burstGap` itself, so a 1 ms gap passed. TS asserts "not at 1 ms". | fixed e8b33be, 82dc1f1 (answer at least 2 ms after arrival, every round) |
| M4 | Med | Kotlin Db Busy had no upper bound, unlike Swift and TS. | fixed e8b33be (reference timer beside, under half the timeout) |
| M5 | Med | "Task to start" sleeps that can FAIL rather than pass vacuously. Swift second receive (WebSocket binding, real URLSession) and second SSE next would hang. Swift Db close would get the wrong error (shown: task started 60 ms late fails). Kotlin CoreCall 150 ms. Kotlin Timer idle 200 ms. | fixed 82dc1f1, 18c9c5b (`running` waits for the task's start; caller parked; ordered reads) |
| M6 | Med | `ci-local --slow` dropped the workspace's clock-reading Rust tests: undra-ffi, runtime, query, transport and the real-time recipe. | fixed 9d3c456 (all but bindgen, macros, cli) |
| M7 | Med | S30 (coordinator, flake on main's CI): a 100 ms absolute wait for the Background cache write. Also step 2 under 1 s, step 4 under 900 ms, and Kotlin cancel under 1 s. | fixed 058aa99 (debounce-relative trials, reference timers, order). Mutant red on TS, Swift and RN; **Kotlin not run** |
| M8 | Med | Other small absolute bounds in the grid: S04, S06, S07, S14, S17, S22, S23, S24, S28. | fixed 058aa99 (relative to the code's constant, a PATCH count, or the 5 s hang detector). **Kotlin and TS full columns not run** |
| L1 | Low | Kotlin trickle comment claimed the count is "the same on any machine". That is not so: the pump and the pull run on different threads there. | fixed (comment); 0 of 400 trials exceeded 20 |
| L2 | Low | `bench/common/stress.rs` (bench harness, not shipped) changed. The stream halves now run at least one round. | accepted |
| L3 | Low | TS realtime-adapters' reference timer uses `QUIET_MS` itself. The fake-clock test pins the value. | accepted |

## Mutations run (each restored; `git status` clean after)

TS:

- busy deadline 2x: fails.
- Busy at once: fails.
- `QUIET_MS` 8 and 1, `LINGER_MS` 16: the fake-clock tests fail. With `QUIET_MS` 8 the realtime-adapters test passes (L3).

Kotlin:

- `QUIET_MILLIS` 1 and 0: burst and lone tests fail.
- `QUIET_MILLIS` 8: lone test fails.
- `MAX_WAIT_MILLIS` 800: trickle fails.
- busy deadline 2x: fails.

Swift:

- transaction statements queued behind waiters: both Db tests fail.
- `burstGap` 1 ms: lone test fails.

Core: Background does not flush. S30 is red on TS, Swift and RN.

## Loops

Kotlin PortsV2Binding: 20 runs normal, 20 under 8 burners, 0 failures. Swift edited classes (88 tests): 20 runs normal, 12
loaded, 0 failures. Burners were checked at 0 after every loop.

## Residual (needs a product seam or is by design)

- S33 polling bounds (2.5 s windows and gap ceilings) need a reference on the core's poll timer: a manual Timer port, or the
  armed interval exposed in stats. A stall of 1 to 1.5 s trips them.
- Swift `running`: it removes the task-start bet, but a preemption in the few instructions between its flag and the pull's
  registration remains. A seam would close it: `PulledInbox` exposing "a pull is pending".
- Negative waits ("for N ms nothing happens") remain everywhere. A slow machine only passes them.

## What remains before merge

1. Run the full Kotlin and TS contract columns, the RN contract column and RN unit tests. Run S30 against the no-flush
   mutant on Kotlin.
2. Run `ci-local --slow --only ci/rust` to completion.
3. Run `cargo fmt` and `cargo clippy --all-targets -- -D warnings`.
4. Merge main, push, and get green CI, Bench, Two cores and Site runs (scenarios.md changed).

Also: during the SIGKILL check this reviewer killed two other worktrees' ci-local runs (a `pgrep` that matched every one).
The coordinator was told at once.
