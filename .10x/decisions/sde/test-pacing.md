# SDE: no test passes or fails because of the machine's speed (wt/test-pacing, 2026-10-02)

A branch now needs a green CI run (about 30 minutes) on its head before it lands, so one test that depends on how fast the
machine is costs a whole cycle. Three such tests had each cost one (the Swift lone-message and trickle tests, the Kotlin
lazy-list restore test, both already fixed); a throttled local pass (`ci-local --slow`, load 80) listed more that had not yet
failed on a hosted runner and could. This piece fixes those and the same classes elsewhere. No assertion about behaviour was
deleted or loosened: where a bound was a number of milliseconds it is now relative to something measured beside the thing under
test, a count, an order, or a configured value; where a wait was a sleep it is a wait on the condition under a deadline that only
detects a hang.

The rules the changes follow: a bound is about the code, not the machine (compare with a reference armed at the same moment on
the same clock, count events, check order, read a configured value); a stimulus that a slow machine cannot deliver (a burst, a
trickle) is recorded and a trial that was not one is repeated, and the assertion is made on one that was; a timeout that is only a
hang detector says so in a comment; and every changed test was broken on purpose (a mutant of the code it guards) to show it still
fails there.

## What each test assumed, what it measures now

| Test | Assumed | Measures now | Mutant that fails it |
|---|---|---|---|
| `bench/tests/stress.rs`: `stress` (debug smoke), `a_skipped_patch_fails_the_equality_invariant`, `a_skipped_view_patch_fails_the_view_invariant`, `a_single_dropped_update_…`, `swapped_change_sets…`, `a_change_set_lost…`, `the_contended_scenario…`, `a_warm_up_runs_first…` | 100 to 200 ms of a scenario do enough operations: the mirror drops its 101st patch (fewer than 101 ran: nothing dropped, an empty list of broken invariants), the stream's fast half runs at all, the writer thread gets a turn ("nothing ran") | `StressConfig::min_ops`: a run goes on past its time until it has done that many operations (smoke: 100; faults: 300 = 30 rounds of ten, at least four patches a round on every mirror; the contended writer: at least one write, or `min_ops`, since the warm-up) and gives up only 120 s after its time (`GIVE_UP`, a hang detector) with what it has; the test says "the machine did not run N operations in T". The stream's halves run at least one round. No upper bound on a run's length remains in `a_warm_up_runs_first…`; it asserts the measured run lasted at least its duration | `time_ops` ignoring the floor: the fault tests fail ("did not run 300 operations (it ran 10)") |
| `runtimes/ts/@undra/runtime/test/support/db-suite.ts`: "carries a 2 MB row whole" (db-node, db-wasqlite twice) | 5 s (vitest default) is enough; `toEqual` over a 1 MiB `Uint8Array` costs 0.9 s of the test's 1 s and prints a million lines on failure | the blob is compared byte for byte (`firstDifference`, 36 ms in all), under an explicit 60 s timeout (a hang detector, said so) | one byte of the expected blob changed: `expected 700000 to be -1` |
| `db-suite.ts`: "a statement on the database during a transaction waits, then fails Busy…" | between a waiting statement and the commit less than the 100 ms busy timeout passes | split: the Busy test (100 ms) as before; the waiting statement runs under a busy timeout of 60 s that never elapses | n/a (removes a race) |
| `db-suite.ts`: "an outer statement waits at most the busy timeout, while the transaction's own statements go on" | the transaction's five statements finish in under 200 ms (`< 200`), and Busy arrives in under 1,200 ms (`< 1200`) | two tests. (1) the five statements are not queued behind the outer one, shown by order under a 60 s timeout (the outer statement is still pending after each; queued, the test hangs to its timeout). (2) Busy at 200 ms (`>= 190`) and before a timer armed for 5 x 200 ms beside the statement fires (overdue timers run in order of deadline: a Busy answered after it is late by the code's doing) | (1) the outer statement holds the queue: `Test timed out`; (2) the wait made 8x: "Busy came before a timer armed for five times…" |
| `test/db-two-cores.test.ts` | the contended write is Busy in under 8 s (13.7 s observed: SQLite's busy handler adds up the sleeps it asked for, not the time that passed) | `PRAGMA busy_timeout` is 5000 on the contended connection (the code's constant, not the machine's), the lower bound (`>= 4500`) is kept, the test's timeout is 120 s (a hang detector) | `busy_timeout = 10000` in `dbPort`: the pragma check fails |
| `test/recovery.test.ts`: the restart test | BEGIN IMMEDIATE returns "at once" = under 1,000 ms | under 2,500 ms (half of SQLite's 5 s busy timeout, which a lock that was not released ends in Busy at: the status check above it); `until` waits on a clock (4 s) and not 200 macrotasks | n/a (a bound tied to a constant of the code) |
| `test/realtime-binding.test.ts`: lone message, trickle (TS counterparts of the Swift tests) | a lone message is answered in under 100 ms; messages one real millisecond apart are less than 2 ms apart (`setInterval(1)`) | both on a clock the test moves (`vi.useFakeTimers`): 2 ms of quiet (not at 1 ms, at 2 ms), and a message a ms until the 8 ms cap answers (more than one, at most nine) | `QUIET_MS = 8` and `QUIET_MS = 0` fail the lone test; `LINGER_MS = 80` fails the trickle |
| `test/realtime-adapters.test.ts`: "a lone message waiting in a receive is answered at once" (real sockets, two adapters) | median round trip under 25 ms, slowest under 100 ms | the adapter is wrapped: a timer of `QUIET_MS` is armed as each message is handed to the binding, and the answer is measured against it (median of 20 under 4 ms, 18th under 100) | a quiet period of 8 ms in `#linger`: 7 ms after the reference, both adapters fail |
| `realtime-adapters.test.ts`: `nodeWebSocket` stops reading | 300 ms is enough for the first socket read to be queued (`early` was a snapshot taken mid-read on a slow machine) | waits until the queue holds something (`until`, 10 s), then the same 500 ms window with nothing more read | n/a (removes a race) |
| `worker-ports`, `snapshot`, `timer-port-start`, `panic-report` | `for 200 x setTimeout(1)` and sleeps of 20 and 80 ms are enough for a worker, a restore, a macrotask and a SHA-256 to finish | `waitFor` (condition, 4 s deadline under the test's 5 s: a hang detector), the eager core's own `called` promise, the reporter's `ready` | n/a (conditions) |
| Kotlin `PortsV2BindingTests`: "a burst is answered as one reply" | six messages pushed with `delay(1)` arrive less than 2 ms apart (they were 3 to 17 ms on a loaded machine: "got 2") | the feed is spun on the clock, `ScriptedInbound.takenAt` records when each message reached the binding, a trial in which two arrived a quiet period apart is not a burst and is repeated (40 tries); the case needs one trial in forty to come back whole (a pull that is not scheduled for 2 ms answers with what it had: the machine's, and a binding that splits a burst does it every time). A first version stopped the feed when the pull answered and hung in 1 of 50 loaded runs: found by the loop, fixed (the feed runs to its end) | `QUIET_MILLIS = 0`: 40 of 40 trials split |
| Kotlin: the lone message | `< 500 ms` (blind to an 8 ms linger) | median of 20 under 5 ms after a 2 ms timer armed on the same event loop at the same moment, both for a waiting pull and a buffered message; tail under 100 ms | `QUIET_MILLIS = 8`: 7.8 ms |
| Kotlin: the trickle | `part.size < 60` with `max` 16: true whatever the binding does (an 800 ms cap passed) | a feeder paced at 0.5 ms on the clock, arrivals recorded, 40 tries; the answer holds more than one message and at most 64 of a `max` of 1,000 | `MAX_WAIT_MILLIS = 800`: "held the pull for 1000 messages" |
| Kotlin: "a transaction id runs inside it; the database id waits for it and is Busy…" | the waiting statement is still within the 200 ms busy timeout after `delay(50)` and the commit | split; the waiting statement has a busy timeout of 60 s | n/a (removes a race) |
| Swift `PortsV2ReviewTests`: `testAnOuterStatementIsBusyAtTheDeadlineAndTheTransactionKeepsGoing` | `waited < 600` with a 400 ms timeout | `>= 395` kept; the end is less than 200 ms (half the timeout) after a `Task.sleep` of 400 ms started beside the statement | the statement's delay doubled: fails |
| Swift: `testTheSwitchToWalWaitsForTheBusyTimeoutThenIsBusy` | `waited < 0.6` with 300 ms | `>= 0.29` kept; less than 150 ms after a 300 ms sleep started beside the open | the retry deadline doubled: `300.9` fails |
| Swift `PortTests`: `testAnAsyncPortDoesNotBlockTheCallerAndSeveralCallsOverlap` | six calls return in under 50 ms (the port's work was a 60 ms sleep) | the six calls are held at a gate the test opens (a 10 s hang detector): none is answered when the callback returned, all six have begun (they overlap), then they are answered | the callback waiting for the work: fails (`6` replies where `0`) |
| Swift `DbBindingTests`: `testOperationsOnOneDatabaseRunOneAtATimeInArrivalOrder` | 20 ms is enough for the slow statement's task to start (lost, the fast one is first) | `eventually`: the slow statement is in the recorder's log | n/a (removes a race) |
| React Native `test/realtime.test.ts`: the binding's lone message | median of five round trips under 25 ms (the same bound as the TS runtime's test) | the same wrapped adapter and quiet-period timer as the TS runtime's test (20 rounds, median under 4 ms) | the quiet period of 8 ms: 6.5 ms after the reference, fails |
| React Native `test/load.test.ts`: the unhandled native failure | 10 ms is enough for the inbox record to be read and reported | waits for the report (4 s, a hang detector) | n/a (a condition) |

## `scripts/ci-local.rb`

* Every `cargo test` of a workflow step runs with `--no-fail-fast` (a substitution, so the workflow files stay the single source):
  one pass lists every failing binary.
* `--slow` no longer runs the heavy `cargo test --workspace` (and the other workspace-wide Rust steps) throttled: it runs the
  timing-sensitive targets alone (`bench/tests`: `-p undra-bench --tests`; the dev-reload tests: `-p undra-cli --test dev
  --test dev_reload --test dev_devtools`; `SLOW_EXTRA`) beside the TypeScript, Swift, Kotlin, contract and React Native steps it
  already ran.
* Nothing runs under `taskpolicy -b` any more: it next to burners at normal priority gave a test no CPU at all. Eight burners
  (`--burners`) at normal priority, four test threads.
* A burner is a shell that runs `yes` and cannot outlive ci-local: it traps EXIT, TERM, HUP and INT and polls its parent, so
  `kill -9` of ci-local, `kill`, a closed terminal and a step that is killed all leave no `yes` (checked: each of those, 4 burners,
  0 left within a second); the step's burners are stopped in an `ensure`, ci-local traps INT, TERM, HUP and QUIT and has an
  `at_exit`.

## Sites read and left alone

Each reads a clock but cannot fail for speed, or a bound is a hang detector sized 10x or more over the stimulus:

* Negative waits (a sleep, then "nothing happened"), which a slow machine passes: Kotlin `CoalesceTests` 920, `StreamTests` 90,
  `ReconnectCoreTests` 108/395, `RemoteReconnectTests` 287/376, Swift `CoalesceTests` 1381, `ReconnectCoreTests` 391/559,
  `PortTests` 185, `AdapterTests` 277, TS `vite.test.ts` 331/392/429, `default-ports` 144, `signal` 118.
* A log cleared after the condition that says everything has landed (so not the lazy-list class): Kotlin `CoalesceTests`,
  `PortsV2BindingTests`, `StoreTests`, `ReconnectCoreTests`; TS `lazy.test.ts` (its `answered()` is a drain on one thread).
* Bounds with a wide margin: Kotlin `HttpAdapterTests` (< 2.5 s against a server that sleeps 3 s and a trickle of 3 s), `StoreTests`
  (observe < 1 s: it would otherwise wait for a core that never answers), `WebSocketClientTests` (about two ping intervals:
  `300..3000` ms for 200), `WebSocketHostileServerTests` (< 2 s against a trickle of 5 s); Swift `RealtimeBindingTests` 298 and
  `RealtimeAdapterTests` 443 (< 1 s, the first duplicated by the reference-timer test); TS `worker-ports` 387 (the test's own 5 s
  against a 60 s start timeout).
* Budget tests (`bench/tests/budgets.rs`, TS `call-path`, `identity`): performance gates with `UNDRA_BENCH_SCALE`, by design.

## Not fixed, and why

* "A task started a moment ago is now waiting" is a 20 to 50 ms sleep in about a dozen Swift tests (`RealtimeBindingTests`
  253/285/307/345/364, `SseBindingTests` 149/182, `PortsV2ReviewTests`' second concurrent receive, `DbBindingTests` 283, …) and a
  few Kotlin ones (`CoreCallTests` 196: a reply 150 ms after the caller suspends; `AdapterTests` 270): there is nothing the test can
  observe that says the task is now waiting (the pending state is the binding's), so no condition to wait on. A task that does not
  start in 20 to 50 ms is a machine stalled that long; none of the loaded loops of this piece (eight `yes` burners) showed one, but
  they ran the touched tests, not these. They are the next thing a slow pass would find, if any: the fix would be a test seam in
  the binding (a probe that says a pull is pending), which is a product change and not made here.
* The burst test's residual: a pull that is not scheduled for 2 ms after it saw a message answers early (its quiet period is
  measured from when it last looked). The recorded arrivals cannot see that, so the case asks for one whole burst in forty.
