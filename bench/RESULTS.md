# Undra benchmark results

Host-measured numbers for every row of the blueprint's section 14 budget table that a host can
measure, the full criterion tables behind them, and what is still waiting for devices.

* Reproduce: `cargo bench -p undra-bench` (criterion, for humans) and
  `cargo bench -p undra-ffi --bench boundary` (the C ABI).
* The CI gate is different and cheaper: `cargo test -p undra-bench --test budgets --release`
  times the same operations with plain `Instant` and fails over `bench/budgets.toml` (see
  [The CI gate](#the-ci-gate)).
* Harsh conditions (sustained load, tails, memory, invariants) have their own gate and a soak:
  `cargo test -p undra-bench --test stress --release` and
  `cargo run -p undra-bench --release --bin soak` (see [Harsh conditions](#harsh-conditions)).
* Medians are criterion's; the bracket is its 95% confidence interval on the median.

## Machine

| | |
|---|---|
| CPU | Apple M5 Pro, 18 cores (Apple silicon, arm64) |
| OS | macOS 26.5 (25F71) |
| Rust | rustc 1.98.1 (48a229cea 2026-09-01), criterion 0.5 |
| Profile | `bench` inheriting `release`: opt-level 3, `lto = "fat"`, `codegen-units = 1`, `panic = "unwind"` |
| Runtime | no `undra-core` thread unless a row says so: the host drives the executor, as the wasm shell does |
| Load | **shared**: other builds ran on this machine during the measurements (load average from 5 to several hundred), so a thread-wake-dependent number can swing; two full runs and the gate's own timings agreed within about 5% for everything single-threaded |

## Section 14 rows, measured on the host

The blueprint's targets are per device (iOS A15-class, Android mid-range 2022, Chromium). An M5 Pro
core is substantially faster than an A15 core, so **"within target" here is necessary, not sufficient**,
and a miss here is a stronger miss on the device. "Host / target" is this host's median divided by the
iOS target.

| Row | Operation measured here | Median | iOS target | Host / target | Verdict |
|---|---|---|---|---|---|
| Handle method call, primitive args and return | `dispatch/call_sync/add` | 43.9 ns | ≤ 60 ns | 0.73x | within on the host, **open for the device** (Finding 2) |
| 1 KB record, round trip (codec) | `wire/record1k/roundtrip` | 228.0 ns | ≤ 3 µs | 0.08x | within |
| 1 KB record, round trip (through a call) | `dispatch/call_sync/echo_record1k` | 139.5 ns | ≤ 3 µs | 0.05x | within |
| Change-set, 100 dirty signals (core side: write, build, deliver) | `signals/changeset_100/runtime` | 2.30 µs | ≤ 100 µs | 0.02x | within |
| Keyed patch on 10,000 items, one insert (recorded list operation) | `signals/keyed_10k/insert` | 6.31 µs | ≤ 20 µs | 0.32x | within |
| Core cold start, 100 KB snapshot restore | `snapshot/cold_start_restore_100kb` | 70.87 µs | ≤ 3 ms | 0.02x | within |
| Core cold start, including the `undra-core` thread | `snapshot/cold_start_restore_100kb_core_thread` | 79.70 µs | ≤ 3 ms | 0.03x | within |
| Web crash recovery, 1 MB state (restore) | `snapshot/restore_1mb` | 270.31 µs | ≤ 100 ms | 0.003x | within |

Two more rows have a host proxy, which is a data point and not a verdict:

| Row | Host proxy | Value | Target | Note |
|---|---|---|---|---|
| Hello-world size added to the app | `undra-ffi` cdylib on `aarch64-apple-darwin`, release, LTO fat, stripped, no app code | 734 KB (316 KB gzipped) | ≤ 900 KB (iOS, arm64) | 82% of the budget before any app code; a macOS dylib carries Mach-O overhead an iOS static link does not. The `undra-ffi` test fixture core (runtime, JNI glue and a test core) is 978 KB stripped. A real measurement needs `aarch64-apple-ios` |
| Runtime memory at idle | resident-set growth of a test process after `Runtime::new` with an `undra-core` thread | about 0.45 MB for the first runtime, about 70 KB for each further one (no core thread) | ≤ 2 MB | `ps` RSS, page-granular, so indicative only; a heap counter needs `unsafe` (R2) |

Also measured, not a row: the same handle method call through the C ABI (`undra_call_sync`,
`boundary/call_sync/add`) is 49.8 ns (79.3 ns before ADR-028), so the ABI itself adds about 6 ns (the
`Runtime::global()` lookup and the `UndraBuf` hand-off). Without the one allocation that hand-off owes, the
same call is 31.5 ns (`dispatch/call_sync_with/add`).

## Harsh conditions

The question a demanding app team asks is not "how fast is one call" but "what happens at a hundred times
any UI's rate, for minutes, with several threads, and does it stay correct and flat in memory". These
scenarios answer it for **the core side** of the pipeline, on a host: a transaction committed, its
change-set built and handed to the host's callback, the host's list updated, a stream and a port call
completed. What a platform does with a change-set afterwards (copy it, decode it, apply it on the main
thread, render) is **not** measured here: it is measured in the browser (the playground's stress screen)
and, for iOS and Android, on the blueprint's devices in the device phase, and the rows for it are empty
until then (see [Device numbers](#device-numbers-ios-android-web) and ADR-031, which is about exactly that
platform half). A host number says "the core is not the bottleneck"; it does not say "the app is smooth".

Numbers below are from `UNDRA_STRESS_SECONDS=10` (best of three runs for the gates, the last run for the
medians) and a 60 s soak, on the machine above. **The machine was shared**: other builds ran throughout (load
average 3 to 8), the cache-bound tails moved by up to 4x between runs (fan-out p99 82 to 295 us), and one
gated run of three needed a second and third attempt for a tail. The gates are 5x to 10x the best run for that
reason; no number was tuned to pass. Two consequences to keep in mind: the baselines come from that loaded machine,
not the quiet one the design asked for, so a quiet run measures better than the `measured_*` values in
`budgets.toml`; and the **absolute** gates catch an operation that became several times slower, not a 2x one (in
review, a commit path made 1.8x slower on purpose passed every layer A row and every sustained gate here; the
ratio gates and baselines of [Gates that catch a 2x regression](#gates-that-catch-a-2x-regression) are what catch
that).

| # | Scenario | What it proves | Measured (10 s run) | CI gate | Verdict |
|---|---|---|---|---|---|
| a | **Firehose**: one observed `Signal<u64>`, one transaction per update, a host calling `call_sync` | each transaction is O(1) in the core at 100x any UI rate: one change-set of exactly 37 bytes | **5.8 M transactions/s**; p50 167 ns, p99 211 ns, p999 295 ns per call (commit + delivery + the copy every FFI callback makes); a core-side burst commits at 79 ns each, 12.7 M/s | at least 1.1 M/s, p99 at most 1.1 us, p999 at most 3 us, 37 bytes; layer A: 1,000-burst 400 us, call 630 ns | within, 5x margin |
| g | **Event firehose**: `Runtime::event` on an event port whose subscriber writes the signal | the path a WebSocket or sensor feed takes into the core | **6.5 M events/s**; p50 125 ns, p99 167 ns, p999 295 ns; 37 bytes | at least 1.4 M/s, p99 840 ns, p999 2.6 us; layer A 580 ns | within |
| b | **Keyed churn**: 10,000 rows, a fixed cycle (4 update, 2 insert, 2 remove, 2 move) at seeded random positions, one operation per transaction, a host list applying every patch | recorded list operations stay O(change) under sustained churn and the host copy never desynchronises | **170 k operations/s**; p50 4.1 us, p99 17.9 us, p999 26.6 us per operation (commit + delivery + host apply); **61 bytes** each; host list equals core list field for field, every operation applied as exactly one patch | at least 33 k/s, p99 90 us, p999 270 us, 61 bytes; layer A 30 ms per 1,000 | within |
| c | **Fan-out**: 1,000 of 100,000 observed signals written per transaction, then the same 1,000 over 10,000 observed (signal-table layer, no runtime) | commit time and bytes follow the dirty count, not the observed count | **22 k transactions/s**; p50 42 us, p99 98 us; one change-set of **21,012 bytes** either way; the 10,000-observed run has p50 27 us (ratio 1.5, gated at 4) | at least 4.6 k/s, p99 410 us, p999 1.3 ms, 21,012 bytes; layer A 190 us and 130 us | within |
| c' | **Fan-out across stores**: 1,000 stores of 100 signals, one written in each, one transaction | the per-store overhead when one transaction touches many stores: 1,000 change-sets sharing a transaction | **7.5 k transactions/s** (7.5 M change-sets/s); p50 100 us, p99 377 us; 33,000 bytes | at least 1.6 k/s, p99 1.3 ms, 33,000 bytes; layer A 580 us | within |
| d | **Stream backpressure**: an always-ready producer, a consumer granting 16 credits a round, then 100,000 | Undra buffers at most one item beyond the consumer's credit, so memory is bounded whatever the producer does | **29 M items/s** with credit; produced minus delivered never above **1** in 7.4 M rounds; RSS **+0.00%** over 10 s | at least 5.7 M/s, RSS at most 1% (or 64 KiB); layer A 180 us per 1,000 items | within |
| e | **Concurrent completions**: 8 host threads answering async port calls, an `undra-core` thread, a 60 Hz "main thread" drain, 256 calls in flight | port calls completed from 8 threads at once are never lost: each wakes its task, commits once and is delivered once, and the drain sees the store's change-sets in transaction order and its total arriving as an exact count (the commits themselves run on the one `undra-core` thread, so this does not contend the per-store delivery lock: that is e') | **339 k completions/s**; call to reply p50 172 us, p99 803 us, p999 1.15 ms; every call answered, every completion one change-set of 37 bytes, **0 lost, 0 out of order**, final total exact | at least 69 k/s, p99 3.7 ms, p999 9.7 ms | within |
| e' | **Contended completions**: scenario e plus a host thread writing the **same store** through `call_sync` as fast as it gets the core lock | ordering when two threads commit to one store (the `undra-core` thread running the completions, and the host thread): every completion and every write adds one to one signal, and the main thread must see that total arrive as exactly `0, 1, 2, .. N` in transaction order, so nothing is lost, repeated or reordered whichever thread committed it. The core lock serialises the two committers, so this tests the hand-off between them and the delivery inside it | **352 k writes/s** in the best of three 10 s runs (about 311 k completions and 40 k host writes a second); completion call to reply p50 143 to 160 us, p99 352 to 426 us, p999 655 to 918 us; the host thread's `call_sync` p50 1.3 us, p99 170 us, max 0.6 to 0.8 ms; every call answered, every write one 37-byte change-set, **0 lost, 0 out of order, 0 steps that were not +1**, final total exact | at least 70 k/s, p99 1.8 ms, p999 6.6 ms, 37 bytes | within |
| f | **Soak**: firehose 100 k/s + churn 20 k ops/s + completions 50 k/s + a stream at 1 M items/s + a 60 Hz drain, together | no leak, and no second whose tail is far off the others' | 60 s: all four loads at 100% of target (see below); RSS **+0.00%** over the second half; every invariant held | RSS at most 1% (or 64 KiB), the p99's trend over the second half rising by at most 50% of its median, worst second's p99 at most 3x the median, invariants; CI runs 10 s | within |

Allocations: one observed single-signal commit allocates **exactly 3 times** (the two vectors of
`group_by_store` and the `claimed` vector), an unobserved one **0**; `crates/undra-ffi/tests/commit_alloc.rs`
holds the first at "at most 3" and the second at 0 with a counting allocator (only that crate may count,
R2). At 100,000 commits a second those 3 are 300,000 `malloc`/`free` pairs a second, which is the next thing
to remove (a roadmap line, not a budget here).

Keyed churn is 170 k operations/s here against the design's 280 k probe (`.10x/specs/2026-09-30-stress-bench-design.md`,
section 3) because the scenario does more per operation, not because the mix is harder: the probe's random
20/20/20/40 mix has the same proportions as the fixed cycle, but the probe used a host that only counted, while
the scenario's host decodes every patch and applies it to its own 10,000-row list inside the timed step. Measured
side by side in review on the same build (load average about 9): 243 k operations/s with the counting host,
151 k/s with the applying one.

### The soak, 60 s

| | Target | Achieved (mean, slowest second) |
|---|---|---|
| firehose, `Ticker.set` through `call_sync` | 100,000/s | 99,999/s, 99,856/s |
| keyed churn, 20 operations per call on 10,000 rows | 20,000 ops/s | 20,000/s, 19,959/s |
| completions, 128 in flight, 8 completer threads | 50,000/s | 49,999/s, 49,916/s |
| stream, the consumer granting 1,000 credits a millisecond | 1,000,000 items/s | 999,987/s, 998,608/s |

6.0 M firehose writes, 1.2 M churn operations, 3.0 M completions, 60.0 M stream items and 10.2 M change-sets
were delivered; the drain thread (60 Hz) found at most 4,280 change-sets in one frame. Every invariant held:
nothing out of order, no completion lost, the store's total equals the number of completions and is the last
value the main thread applied, the host's 10,000-row list equals the core's after 1.2 M patches, the stream
at most one item ahead of its credit at the end (scenario d checks it after every round), no warning logged. **RSS** (`ps`, sampled once a second): 10.39 MB
at 1 s, 10.52 MB at 6 s, 10.58 MB from 11 s to the end, so +0.00% after the half-run warm-up. The firehose's own
p99 inside the mix is 59 us (median over the seconds, worst 76 us), not the 211 ns it has alone: it is the
wait for the core lock behind a 20-operation churn call or a completion burst, which is what mixed load costs;
the number itself is not gated: the soak's latency gates compare the post-warm-up seconds with each other (below).

**The drift gate.** Until this commit the soak's latency gate was a spike gate (the worst post-warm-up second's p99 at
most 3x the median second's), which a steady climb passes: a p99 that doubles over the second half has a worst/median of
1.33. It now has two parts. The spike gate stays. Next to it, a straight line is fitted through the per-second p99s
after the warm-up (the second half), and the line may not rise by more than **half of the median p99** across them
(`undra_bench::stats::drift`). The fit is Theil-Sen, the median of the slopes between every pair of seconds, not least
squares: one bad second at the end of the window tilts a least-squares line (a flat 40 us series with one 400 us
second at the end rises by 40% of its median under least squares and by 0 under Theil-Sen), and that second is the spike
gate's business. A trend is fitted to at least six seconds, which a 10 s run has exactly. Evidence, all in the unit tests
of `bench/src/stats.rs` and on the real soak:

* a synthetic p99 that climbs steadily, 40 us to 100 us over 30 seconds with 8% jitter, rises by 80% to 140% of its
  median and **fails**; one that doubles steadily (40 us to 80 us, the review's case) **fails** and passes the old gate;
  a flat series with 25% jitter, a 20% climb with 10% jitter and a falling series pass; two bad seconds at the end do
  not tilt it;
* the second half (30 to 60 s) of a real 60 s soak on this host (31 seconds, 57 to 125 us) fits at -5% and **passes**
  (the series is `REAL_SOAK_SECOND_HALF` in the tests); the 60 s soak with this gate passed at -5% of the median;
* a commit made to slow down steadily while the soak runs (a spin of 60 iterations per second of uptime added to every
  commit, never committed: firehose p50 251 ns at 1 s, 1.15 us at 10 s, 3.39 us at 60 s) leaves the p99 rising from 57 us
  to 135 us. Every other gate passes it (the spike gate at 1.6x the median, RSS +0.43%, every rate at 100%); the drift
  gate fails it: "a rise of +56% of the median across 31 seconds after warm-up".

The limit is the host, not the gate: this machine's scheduler moves the firehose thread between fast and slow cores, and
the p99 flips between a regime around 40 us and one around 110 us, in one 60 s run at 7, 11, 15 and 27 s. A step that
falls inside the second half and does not return reads as a climb (tested: 40 us then 110 us fails). That is why CI runs
the soak with `--attempts 2`, each attempt a fresh process that lands in its own regime, and why the gate reads the
second half only.

RSS on macOS climbs in page-sized steps early under this load (allocator magazines and thread stacks
settling): 10.39 MB at 1 s, 10.52 MB at 6 s, 10.58 MB from 11 s on in the run above, and in two of the four
60 s runs that counted only the first 20% as warm-up it took further steps at 17 to 33 s and ended at +0.88% and
+2.05%. It is flat afterwards, so the soak's warm-up is the first half of the run; both 60 s runs with that rule
ended at +0.00%, and the 10 s CI configuration (5 s of warm-up) passed five runs in a row, after one of an
earlier six ended at +1.44% and one missed its completion rate while another build ran. That is the host
allocator, not the core, and it is why CI runs the soak with `--attempts 2`: a run that fails only a noisy gate
is repeated, a broken invariant is not.

### Method

* **Warm-up** is excluded and configurable. Every sustained scenario first runs for a warm-up, results
  discarded (thread start-up, cold caches, the first allocations): a tenth of the measured run, at most 200 ms, so a
  10 s run warms up for 200 ms and a 2 s CI run for 200 ms. `UNDRA_STRESS_WARMUP_MS` sets it (`0` measures from the
  first operation); each result file records the warm-up it used. The measured run is then exactly
  `UNDRA_STRESS_SECONDS`; throughput, percentiles and bytes are of that run only, while the invariants cover both (a
  patch lost during the warm-up still fails). The stream has no warm-up of its own: its slow half and the 20% RSS
  warm-up do that job. Before this change no scenario excluded one, which at CI's 2 s put thread start-up and the
  first cold calls into the completions histogram. The soak's warm-up is `--warmup PCT` (default half the run: the RSS
  and drift gates look only at what follows; the rate gate counts every second).
* **Sustained throughput** is operations divided by the wall time of the measured run, not the sum of
  per-operation samples. **Latencies** time every operation with one `Instant` pair into a fixed log-linear
  histogram (1/32 relative error, no allocation per sample), so p999 is of millions of samples; a percentile is
  the upper bound of its bucket. `Instant` ticks at 41.67 ns on this host, so a 167 ns p50 is four ticks, and
  the sub-microsecond percentiles are whole ticks (a 211 ns p99 is five, give or take one). Timing every
  operation costs about 35 ns of wall time per sample here (an empty timed step runs at 28 M/s), which the
  throughput includes: the firehose's 5.8 M/s is the rate with that clock (and the host's copy) in the loop,
  against 8 M/s for the same call timed in batches with a host that only counts (layer A, 125 ns).
* **Bytes** are what the host's callback was handed (`payload.len()`) and are deterministic: every load is a fixed
  cycle at fixed widths. The gate in `budgets.toml` (`bytes_per_op`) is a **ceiling**, not an equality: a run that
  ships fewer bytes passes it. What makes it exact is each scenario's own invariant in code, which asserts the exact
  count (37 bytes per change-set for the firehose, event and both completions scenarios, 61 per operation for
  churn, 21,012 and 33,000 per transaction for the fan-outs); a scenario that shipped fewer bytes fails there.
* **Memory** is resident set size (`/proc/self/status` on Linux, `ps -o rss=` on macOS, nothing else), sampled
  outside the timed region and page-granular (16 KiB here), so the gate is "at most 1% **or** 64 KiB" from the
  first sample after the warm-up to the last. A platform that cannot be sampled reports the gate as skipped, never
  as passed. **A flat RSS is weaker evidence on macOS than on Linux**: `ps`'s `rss` leaves out the pages the system has
  compressed, so under memory pressure a leak that was written once can be compressed away and the line stays flat.
  `phys_footprint` is the right metric there, and it needs `proc_pid_rusage` (FFI), which R2 keeps out of this crate.
  Both the stress test and the soak print this next to their RSS verdict; trust the 60 s soak over the 10 s one, and
  prefer a Linux run (CI) for a verdict on small leaks. The exact check is an allocation-balance test in `undra-ffi`
  (review, "A stricter memory check"), which is not built.
* **Invariants** are asserted in code and are not scaled or retried: a fast core that loses or reorders a
  change-set fails. They have teeth: tests make the host drop every 101st patch, drop one single patch (an
  update that a later write of the same row repairs, so only the count of applied patches sees it), and make
  the main-thread model swap two change-sets or lose one on its way to the UI (in scenarios e and e'), and
  require the equality, patch-count, order, loss and exact-count invariants to fail. In e and e' the store's
  total is a signal every write adds one to, so the main thread checks that it arrives as `0, 1, 2, ..` with no
  step that is not +1, which catches a lost, repeated or reordered change-set in one test. The one timing comparison a scenario makes about itself (fan-out over 100,000 observed at most 4x the
  10,000 case) is a gate, retried like the others, not an invariant.
* **Host stand-ins**: a copy of every change-set (what each FFI callback does), a host list that decodes and
  applies each keyed patch, and a "main thread" that drains once a frame and checks per-store order. They stand
  in for the platform's mailbox and list state; they cost far less than the platform's own work does.
* **Where a number comes from.** The harness writes the JSON behind a run when `UNDRA_BENCH_RESULTS_DIR` names a
  directory: one file per sustained scenario (`<date>-<scenario>.json`), one for the layer A rows and ratio gates
  (`<date>-layer-a.json`) and one for a soak (`<date>-soak-<N>s.json`), each with the numbers, the command, the machine
  (CPU, cores, OS, compiler, commit) and the one-minute load average before and after. The published numbers
  are those files, in `bench/results/`, and the sections above name them; CI uploads the same files of every run as an
  artifact. `UNDRA_BENCH_RESULTS_TAG=name` puts a tag in the file names for a run that is evidence and not a published
  number. The `measured_*` values in `budgets.toml` are different: the best of three runs at the time a gate was set,
  kept for humans and checked to be inside their gates.
* **Budgets** follow the file's rule: floor = measured / 5, p99 ceiling 5x, p999 ceiling 10x, bytes exact,
  RSS 1%; each layer A row 5x its p50. Reproduce: `UNDRA_STRESS_SECONDS=10 cargo test -p undra-bench --test
  stress --release -- --nocapture`, `cargo run -p undra-bench --release --bin soak -- --seconds 60`,
  `cargo bench -p undra-bench --bench stress` (criterion, for humans).

### Gates that catch a 2x regression

The absolute budgets are 5x what the reference host measures (CI doubles that with `UNDRA_BENCH_SCALE=2`), so
that a slower runner passes. That is also why they cannot see a regression of 2x. The bench review (M1) made the
commit path 1.8x slower on purpose, with a spin in `undra_signals::txn::commit`, and every gate passed. Repeated here
on the reference host (load average about 2), the spin adds about 63 ns to each commit:

| Row | Before | With the commit 1.8x slower | Absolute budget | Margin left |
|---|---|---|---|---|
| `stress/firehose/txn_x1000` (a commit and its delivery, 1,000 times) | 78.2 us | **137.7 us** (1.76x) | 400 us | 2.9x |
| `stress/firehose/call_set` | 136.2 ns | 224.2 ns | 630 ns | 2.8x |
| `stress/firehose/event` | 115.8 ns | 207.0 ns | 580 ns | 2.8x |
| every other layer A row | | unchanged | | 5.1x to 6.2x |
| `firehose/sustained` (4 s runs) | 5.9 M/s | 3.90 M/s | floor 1.1 M/s | 3.5x |

Two gates that do not depend on how fast the machine is close that, and a third mechanism makes the second usable on
CI.

**Ratio gates** (`[ratio."..."]` tables in `budgets.toml`) compare two layer A rows measured **in the same run**, so
the machine's speed cancels. Each `max` is 1.15x the largest ratio seen on any healthy sample (the reference host's
runs and six runs of the Bench workflow on a GitHub runner, Oct 2026), rounded up to 0.1.

| Gate | Rows | Healthy, host and six runner runs | `max` | With the commit 1.8x slower on the host |
|---|---|---|---|---|
| `commit_vs_bare_call` | `txn_x1000` per transaction / `dispatch/call_sync/add` | 1.26 to 1.94 | 2.3 | 2.95 to 3.4 (**fails**) |
| `set_vs_bare_call` | `call_set` / `dispatch/call_sync/add` | 2.30 to 3.05 | 3.6 | 4.9 to 5.1 (**fails**) |
| `event_vs_set` | `event` / `call_set` | 0.76 to 0.93 | 1.1 | 0.90 to 0.99 (passes: the commit is in both) |
| `fanout_100k_vs_10k_observed` | the two fan-out rows | 0.99 to 1.92 | 2.3 | 1.4 (passes: commit cost follows the dirty count) |
| `keyed_churn_vs_set` | `keyed_churn_10k` per operation / `call_set` | 15 to 47 | 54 | 26 (passes) |

A ratio can only see a shift larger than **its own spread across hardware**, and the six runner runs show how large
that is. The ubuntu-latest pool is not one machine class: the same row measured up to 2.2x apart between runs
(`stress/firehose/txn_x1000` 123 to 275 us, `signals/changeset_100/cell` 1.56 to 4.44 us), in two clusters. Rows of one
family (a commit against a bare call) keep their ratio within 1.2x to 1.5x; rows of different families (a memmove-bound
list operation against an atomics-bound call: the runner runs the list 1.2x slower than the host and the call 3x slower)
within 2x to 3x, which is why `keyed_churn_vs_set` is wide and guards an order-of-magnitude shift on a runner, not a 2x
one. Only the two commit-versus-call ratios are tight enough to see a commit that is 2x slower, and a uniform slowdown
of the whole machine is, by construction, invisible to a ratio. `ratios_hold_on_every_recorded_sample` in
`bench/tests/budgets.rs` holds the recorded samples (the runs are named in its comment) and checks that every gate
passes on every one; the next two tests model a commit `S` times slower on the same samples (a transaction costs `S`
times as much, and `call_set` and `event` pay the difference once). At 2x, `commit_vs_bare_call` fails on all eight
samples; at 1.8x on seven, and the one it misses is the fastest runner run (2.26 against a maximum of 2.3).

**Baselines** (`bench/baselines/<name>.toml`, `UNDRA_BENCH_BASELINE=<name or path>`) record what one machine class
measured for every layer A row and sustained scenario, and fail a run on that class that is more than 1.5x worse
(p50 1.5x, or 20 ns more where that is more for rows of a few tens of nanoseconds; a throughput under 1/1.5; a p99 over
2.5x). `UNDRA_BENCH_RECORD=path` records one (best of three attempts, `UNDRA_BENCH_RECORD_BEST=1` keeps the better
of what the file holds), with the CPU, the load average and the commit next to the numbers.
`bench/baselines/apple-m5-pro.toml` is the reference host's. It catches the 1.8x commit on the host: `txn_x1000` at
1.80x its baseline, `call_set` 1.53x, `event` 1.68x, `firehose/sustained` 3.85 M/s against a floor of 4.17 M/s, and
`event/sustained` 4.58 M/s against 5.04 M/s. `UNDRA_BENCH_SCALE` does not apply to a baseline.

**On CI the baseline is the merge base, measured in the same job.** A baseline recorded on one machine is worth
nothing on another, and the runner pool is at least two machines, so `bench.yml` records the baseline from the
**base commit on the same VM** with `scripts/bench-record-base.sh` (this tree's harness against the base's crates, in
a target directory of its own, best of three attempts per row, 2 s per scenario), then gates the head against it. For
a pull request the base is the first parent of the merge ref; for a push it is the commit the push replaced. The same
machine, minutes apart: its speed cancels by construction. Run end to end on the reference host:

| | Rows against the base recorded 100 s earlier | Ratio gates | `firehose/sustained`, `event/sustained` |
|---|---|---|---|
| the head unchanged | `txn_x1000` 1.03x, `call_set` 1.05x, `event` 1.04x, all others under 1.5x | pass (1.76 and 2.85) | 6.14 M/s, 7.29 M/s: pass |
| the head with the commit 1.8x slower | `txn_x1000` **1.89x**, `call_set` **1.82x**, `event` **1.84x** | **fail** (3.38 and 5.11) | 4.02 M/s, 4.50 M/s against floors of 4.11 and 4.91: **fail** |

What none of this sees: a regression under 1.5x (the baseline) or under a ratio's own spread, a regression in a row the
base does not have, and anything when the base does not build against the head's harness (the step then says so and the
gates are the absolute budgets and the ratios). The first CI run of this workflow is the first time `bench-record-base.sh`
runs on a runner.

### What these numbers are not

They are a host's core side, not a device's, and not the platform half. The same firehose reaches the UI
as 100,000 change-sets a second to copy, decode and apply (and a keyed patch copies the whole list on
TypeScript and Kotlin), which is what ADR-031 changes; the before and after on the same screen, in the
browser and on the devices, will be recorded here. Until then the only claim these rows support is the one
they measure: **the core commits, delivers and keeps order and memory at rates 100x any UI's.**

## Findings

### 1. A keyed patch was O(list), not O(change); with recorded list operations it is O(change) (resolved, ADR-027)

The blueprint row says lists must be "O(change), not O(list)". It was not: the cost was linear in the list and
independent of the change, and the 10,000-row insert missed its row by ~27x. Before and after, the same
operation through the runtime (`signals/keyed_*`, criterion medians):

| Rows | One insert | One update | One move | Per row (insert) |
|---|---|---|---|---|
| 100, before | 6.28 µs | | | 63 ns |
| 1,000, before | 55.74 µs | | | 56 ns |
| 10,000, before | 536.16 µs | 532.32 µs | 702.95 µs | 54 ns |
| 100, recorded | 359.1 ns | | | |
| 1,000, recorded | 769.7 ns | | | |
| **10,000, recorded** | **6.31 µs** | **272.3 ns** | **9.74 µs** | 0.6 ns |

**The cause** was the design that `StoreCell`'s docs described ("each commit costs O(n) to compute the patch"): at every
commit `KeyedList::diff` (`crates/undra-signals/src/store.rs`) called `KeyedPatch::diff`
(`crates/undra-wire/src/patch.rs`) over the old and the new list, and that diff hashes every key of both lists into two
`HashMap`s and builds position tables before it looks at what changed. Measured on its own, with a plain `u64` key and
`PartialEq` (`wire/keyed_patch_10k/diff`), it takes 456.79 µs: **85% of the 536.16 µs**; the rest was the generated key
function, the encoded comparison of surviving rows and the baseline replay.

**The fix** (ADR-027) records the operation instead of rediscovering it. `Signal<Vec<T>>` has `push`, `insert`, `remove`,
`update_at`, `move_item` and `clear`, which mutate the list and append the SPEC 3.8 op they performed to a log; a commit of
a list written that way sends the log as the patch and replays it on the baseline, O(ops), with no key hashing and no item
comparison. The wire is unchanged. The benchmark fixture's store methods now use them, which is what generated store code
does. What remains at 10,000 rows is the `memmove` of the vector's tail, which the core's own `Vec::insert` needs and which
is paid twice (the list and the baseline): 0.6 ns per row, so an insert into the middle of 10,000 rows is 6.3 µs, and an
update, which moves nothing, is 272 ns (dispatch, argument decode, the write and the patch). The host side is unchanged
and cheap: decoding a one-op patch and replaying it on a 10,000-row list is 2.57 µs (`wire/keyed_patch_10k/apply`).

**The fallback** is unchanged on purpose. A list written with `set`, `update` or `replace` (a whole-list refresh, or an
edit no recorded operation can express) is still found by diffing it against what the host has: the same one-row edit
through the raw `update` is `signals/keyed_10k/raw_update_diff`, 528.7 µs in the gate harness (534.7 µs before, same
machine and load), and a transaction that mixes recorded operations with a raw write is diffed as a whole. Its budget is
guarded at the old level; the two paths are different rows so neither can hide a regression in the other.

### 2. The handle method call: the allocator was 60% of it, ADR-028 took it out, the row is within target on the host

The row was the second miss: `Runtime::call_sync` of `add(i64, i64)` was 77.8 ns when first measured (1.30x the 60 ns
target) on a core faster than the iOS device's. A sampling profile of the call (macOS `sample`, ~4,900 samples of the
loop) put **about 60% of it in the allocator**: `malloc` and `free`, plus a `mach_absolute_time` read that this OS's
`libmalloc` does inside every allocation (the top symbol, 23% of the samples). The call allocated at least three times:
the dispatcher's encoded return value (`encode_to_vec`), its `Box<dyn Any>` outcome, and the `Reply` payload `Vec`.

ADR-028 removes all three. A generated dispatcher answers a synchronous method with `rt.sync_ok(&value, Encode::encode)`
(`sync_err` for a typed error): when `call_sync` has armed the thread's reply slot, that encodes the whole `Reply` payload
(`call_id`, status, value) into one reusable thread-local buffer and returns a zero-sized outcome (boxing a zero-sized
value does not allocate); `Runtime::call_sync_with` lends the buffer to a closure. `Runtime::call_sync` copies it into the
`Vec` it returns, which is the one allocation left and the one the C ABI owes (`undra_call_sync` hands that `Vec` over as
the `UndraBuf` the caller frees). Whatever the buffer cannot serve (`undra_call`, dispatch layers, a call nested in another,
a second runtime) takes the old allocating path, byte-identical on the wire. `crates/undra-ffi/tests/sync_alloc.rs` counts
allocations with a global allocator and holds the path to exactly 0 per `call_sync_with`, 1 per `call_sync` and 1 per
`undra_call_sync`. Four smaller costs on the same path went with it: SipHash on the `u32` dispatch ids, a linear scan of
the object's method list per call, an `Arc` reference taken twice per receiver lookup, and several thread-local accesses
in the panic guard.

The parent commit was built in a second worktree and measured back to back with the new build (criterion medians, best of
three interleaved rounds each; the machine was shared, load average 5 to 20, and the single-threaded rows agreed within
about 3% between rounds when it was not saturated):

| Benchmark | Before | After |
|---|---|---|
| `dispatch/call_sync/add` (returns a `Vec`) | 73.8 ns | 43.9 ns |
| `dispatch/call_sync_with/add` (new: no allocation at all) | n/a | 31.5 ns |
| `dispatch/call_sync/function` | 65.4 ns | 35.8 ns |
| `dispatch/call_sync/echo_record1k` | 268.4 ns | 139.5 ns |
| `boundary/call_sync/add` (C ABI, one `UndraBuf`) | 79.3 ns | 49.8 ns |
| `boundary/call_sync/unknown` (status 5: formats a reason, allocates by nature) | 106.8 ns | 107.3 ns |
| `boundary/call/add` (the async entry; the slot is not armed there) | 102.3 ns | 99.2 ns |

What is left of the 31.5 ns, from a sampling profile of `call_sync_with`: about a quarter is entering and leaving the core
lock (re-entrancy check, the lock, the current-runtime scope with its `Weak::upgrade`), about a sixth the two receiver
lookups (the object table's reader lock and an `Arc` reference, once to route and once for the dispatcher), a tenth the
panic guard, and the rest payload decode, the dispatcher and the reply encode. The cheapest next steps are resolving the
receiver once per call instead of twice, and a cheaper re-entrancy check (one thread-local read instead of two);
neither changes a contract.

**The row is not closed.** 43.9 ns against 60 ns is a pass on a core faster than an A15, which is necessary and not
sufficient: the row passes on the device only if an A15 core runs this path within 1.37x of this core's time (60 / 43.9),
and this note has no A15 to measure. The verdict belongs to the device phase. `undra_call` (the async entry) still
allocates its reply; it is not on the synchronous row's path.

### 3. `Runtime::new` + drop, without `shutdown()`, leaks the runtime and two threads

Found while building the cold-start benchmark: a `Runtime` created with `Runtime::new` and merely dropped is never
freed, and its `undra-core` thread and a second thread stay alive (100 create/drop cycles: +200 threads; after
about 1,500 cycles thread creation on this host degrades from ~100 µs to several ms). `shutdown()` before the
drop fixes it (+0 threads, runtime freed). The cause is a reference cycle through the `undra-query.hydrate`
init hook: it spawns a task that holds a `Ctx` and parks on the unavailable `Kv` port, so the executor owns a
task that owns the runtime. `TestRuntime` and `undra-transport`'s server both call `shutdown()`, so nothing in
the tree trips over it, and `Runtime::shutdown` documents that surviving `Ctx`s keep a runtime alive; but an
embedder that relies on drop leaks silently. The bench harness wraps its runtimes so they are shut down.

### 4. Expensive wire types

Everything scalar, string, option, enum and record round-trips in tens to a few hundred ns, and the 1 KB record in 228.0 ns.
The two that stand out are maps: a 100-entry `HashMap<String, u32>` is 6.46 µs and `HashMap<u32, u32>` is 3.26 µs round trip, dominated by
the encode (3.30 µs and 2.48 µs): maps encode their entries sorted by the encoded key bytes, so every encode encodes each key
into a scratch buffer and sorts. `Vec<u32>` of 1,000 is 1.12 µs (0.63 ns per element to decode): it goes element by element rather than as one copy. None is
near a section 14 row; they are the first places to look if a large-collection command ever shows up in a profile.

## Full tables

### Wire: encode, decode and round trip per type

`encode` writes into a reused buffer (the codec alone); `decode` reads a fixed byte string into an owned
value; `roundtrip` is what a call argument or return value pays: `encode_to_vec` (one allocation) and
`decode_exact`.

| Type | encode | decode | round trip |
|---|---|---|---|
| `bool` | 0.95 ns | 1.00 ns | 14.7 ns |
| `u8` | 0.95 ns | 0.95 ns | 15.5 ns |
| `u32` | 0.95 ns | 0.97 ns | 15.5 ns |
| `i64` | 1.16 ns | 0.72 ns | 15.6 ns |
| `f64` | 1.96 ns | 0.74 ns | 16.2 ns |
| `string_short` | 3.05 ns | 20.5 ns | 36.1 ns |
| `string_1kb` | 11.1 ns | 49.1 ns | 79.5 ns |
| `bytes_1kb` | 11.0 ns | 28.6 ns | 56.8 ns |
| `option_some` | 1.56 ns | 0.88 ns | 16.5 ns |
| `option_none` | 1.43 ns | 1.07 ns | 15.8 ns |
| `vec_u32_1k` | 258.2 ns | 630.1 ns | 1.12 µs |
| `map100_string_u32` | 3.30 µs | 2.81 µs | 6.46 µs |
| `map100_u32_u32` | 2.48 µs | 770.4 ns | 3.26 µs |
| `duration` | 1.17 ns | 1.04 ns | 16.9 ns |
| `timestamp` | 1.14 ns | 0.77 ns | 15.5 ns |
| `uuid` | 0.96 ns | 1.33 ns | 16.2 ns |
| `record5` | 4.50 ns | 22.3 ns | 96.7 ns |
| `record1k` | 13.6 ns | 68.7 ns | 228.0 ns |
| `enum_rect` | 1.84 ns | 3.02 ns | 56.3 ns |
| `enum_label` | 4.77 ns | 19.9 ns | 53.0 ns |
| `result_ok` | 1.99 ns | 2.71 ns | 16.9 ns |
| `result_err` | 3.43 ns | 23.5 ns | 68.5 ns |

### Wire: keyed patch (undra-wire)

The patch algorithm and its host-side replay on their own, with a cheap key and `PartialEq`. `diff` is the O(list) fallback that a raw write (`set`, `update`, `replace`) takes since ADR-027; a recorded list operation does not run it. Before ADR-027 it was 85% of the keyed signals rows (Finding 1).

| Benchmark | Median | 95% CI |
|---|---|---|
| `wire/keyed_patch_10k/apply` | 2.57 µs | 2.51 µs .. 2.63 µs |
| `wire/keyed_patch_10k/diff` | 456.79 µs | 450.62 µs .. 464.15 µs |
| `wire/keyed_patch_10k/roundtrip` | 136.7 ns | 134.4 ns .. 139.3 ns |

### Dispatch

`Runtime::call_sync` / `Runtime::call` with a prebuilt payload and a host that only counts: `undra_call_sync` without the C ABI. `call_sync` returns the reply as a `Vec` (the one allocation the C ABI owes the host); `call_sync_with` lends the reply buffer instead and allocates nothing (ADR-028). `call_async` includes building the `Call` payload (a host must) and running the executor (`run_pending`) on this thread; there is no thread hop.

| Benchmark | Median | 95% CI |
|---|---|---|
| `dispatch/call_sync/add` | 43.9 ns | 43.8 ns .. 44.0 ns |
| `dispatch/call_sync_with/add` | 31.5 ns | 31.4 ns .. 31.6 ns |
| `dispatch/call_sync/function` | 35.8 ns | 35.6 ns .. 36.0 ns |
| `dispatch/call_sync/echo_record1k` | 139.5 ns | 139.2 ns .. 139.8 ns |
| `dispatch/call_async/ready_add` | 220.6 ns | 217.6 ns .. 224.8 ns |

### Signals and stores

`cell` is the signals crate alone (100 `Signal<u32>` attached to a `StoreCell`, one transaction, a counting sink). `runtime` is the same 100 writes as one method call on a macro-generated store through the runtime. `decode` is a host validating and walking that change-set (borrowed). Keyed rows are one call through the runtime on an observed `Signal<Vec<Item>>` with `#[undra(key = "id")]`, written with the recorded list operations (`insert`, `update_at`, `move_item`); `raw_update_diff` is the same one-row edit through the raw `update`, which takes the diff path. Insert runs against a list that is restored outside the timed region. The wide interval on `raw_update_diff` is machine load (the gate harness measured 528.7 µs p50).

| Benchmark | Median | 95% CI |
|---|---|---|
| `signals/changeset_100/cell` | 760.9 ns | 749.6 ns .. 771.4 ns |
| `signals/changeset_100/runtime` | 2.30 µs | 2.27 µs .. 2.33 µs |
| `signals/changeset_100/decode` | 253.9 ns | 252.6 ns .. 255.1 ns |
| `signals/observe_100_initial` | 2.82 µs | 2.80 µs .. 2.86 µs |
| `signals/keyed_10k/insert` | 6.31 µs | 6.25 µs .. 6.35 µs |
| `signals/keyed_10k/update` | 272.3 ns | 271.5 ns .. 273.5 ns |
| `signals/keyed_10k/move` | 9.74 µs | 9.62 µs .. 9.86 µs |
| `signals/keyed_10k/raw_update_diff` | 579.7 µs | 547.9 µs .. 630.8 µs |
| `signals/keyed_1k/insert` | 769.7 ns | 745.8 ns .. 793.2 ns |
| `signals/keyed_100/insert` | 359.1 ns | 352.6 ns .. 366.0 ns |
| `signals/computed/recompute_1` | 42.1 ns | 41.8 ns .. 42.4 ns |
| `signals/computed/recompute_chain_10` | 253.1 ns | 251.0 ns .. 256.3 ns |

### Snapshot and restore

100 KB is four stores of 250 rows of 100 bytes; 1 MB is forty. `cold_start` builds a runtime and restores; the runtime it made is shut down outside the timed region.

| Benchmark | Median | 95% CI |
|---|---|---|
| `snapshot/encode_100kb` | 13.79 µs | 13.73 µs .. 13.87 µs |
| `snapshot/restore_100kb` | 26.88 µs | 26.66 µs .. 27.15 µs |
| `snapshot/restore_1mb` | 270.31 µs | 268.02 µs .. 273.25 µs |
| `snapshot/cold_start_restore_100kb` | 70.87 µs | 70.12 µs .. 71.58 µs |
| `snapshot/cold_start_restore_100kb_core_thread` | 79.70 µs | 78.65 µs .. 80.96 µs |

### C ABI (`crates/undra-ffi/benches/boundary.rs`)

`call/ready_add` is the only row that includes a real `undra-core` thread hop (spawn, wake, poll, reply on the core thread) and is the one most sensitive to machine load.

| Benchmark | Median | 95% CI |
|---|---|---|
| `boundary/call_sync/add` | 49.8 ns | 49.6 ns .. 50.0 ns |
| `boundary/call_sync/unknown` | 107.3 ns | 106.7 ns .. 108.1 ns |
| `boundary/call/add` | 99.2 ns | 98.7 ns .. 100.1 ns |
| `boundary/call/ready_add` | 5.64 µs | 5.50 µs .. 5.80 µs |
| `boundary/write_observed` | 129.7 ns | 129.3 ns .. 130.2 ns |

### Query client (`bench/benches/query.rs`)

No row of its own in section 14; kept so regressions in the hot paths are visible.

| Benchmark | Median | 95% CI |
|---|---|---|
| `query/observe_cached_and_release` | 516.2 ns | 512.0 ns .. 521.1 ns |
| `query/refetch_published_to_100_observers` | 6.90 µs | 6.82 µs .. 6.98 µs |
| `query/platform_construct_and_release` | 1.19 µs | 1.18 µs .. 1.22 µs |
| `query/platform_refetch_call` | 586.1 ns | 585.0 ns .. 587.4 ns |

## The CI gate

`bench/budgets.toml` holds a host budget for each of the 55 operations the gate runs (the wire round trips,
dispatch, signals, snapshot and the eight per-operation rows of the harsh-conditions scenarios). Each is about **5x** what this machine measures (with a 250 ns floor and two
significant figures), which is what makes a shared CI runner pass while an operation that became several
times slower fails. The budgets guard against **regressions on a host**; they are not the section 14 device
targets, and none of them sits above its device target today. (The keyed-patch rows were one miss until
ADR-027: the insert row's gate is 31 µs, 5x what it measures, and what it measures, 6.2 µs, is under the
20 µs device target. The handle method call was the other until ADR-028: its budget now guards the 43.9 ns
it measures, not the allocator.) The test takes the best p50 of up to three attempts, runs in `--release` only (a debug build
just smoke-runs every operation, so `cargo test --workspace` stays green and fast), and supports
`UNDRA_BENCH_SCALE` for a slower runner. `.github/workflows/bench.yml` runs it on every PR and on main.

Two more steps follow it in the same job. `cargo test -p undra-bench --test stress --release` runs the eight
sustained scenarios of [Harsh conditions](#harsh-conditions) for 2 s each and fails over their
`[stress."..."]` tables in `budgets.toml` (a throughput floor, p99 and p999 ceilings, exact change-set bytes,
RSS growth) or on a broken invariant (nothing lost, nothing reordered, the host's list equals the core's, a
stream never more than one item ahead); a noisy run gets three attempts, an invariant none. Then
`cargo run -p undra-bench --release --bin soak -- --seconds 10 --attempts 2` runs the mixed paced load and fails
on RSS growth, drift in the firehose's p99, a broken invariant or a host that could not carry the load. The
debug build of the stress test checks invariants only (500 churn rows, 100 ms per scenario), so
`cargo test --workspace` stays green and about 4 s slower.

Before those steps `bench.yml` records a baseline from the base commit on the same VM, and the budgets and stress
steps gate against it as well (see [Gates that catch a 2x regression](#gates-that-catch-a-2x-regression)); the
ratio tables of `budgets.toml` run in every budgets step on every machine.

Every run uploads the JSON behind its numbers (`UNDRA_BENCH_RESULTS_DIR`, see Method) as the `bench-results`
artifact; the soak runs each attempt in a fresh process.

## Device numbers (iOS, Android, Web)

> The playground has now been exercised interactively on the iOS simulator, the Android
> emulator (live 10/s keyed-patch streaming on the 10,000-row list) and Chrome. Those runs
> validate behaviour, not budgets: virtualized numbers are deliberately NOT recorded here —
> only real-device measurements will fill this table, so the budget verdicts stay honest.
> Per-platform benchmark splits are likewise deferred until real devices produce them.

Land with the playground phase, measured on the devices the blueprint names (iPhone with an A15, a 2022
mid-range Android phone, Chromium) from `examples/playground`. Until then:

| Row | Waiting for |
|---|---|
| Handle method call, all three platforms | the real Swift/JNI/JS crossing on device: the host number above is the core half only |
| 1 KB record round trip, all three | the same, plus the platform runtime's own encode/decode (Swift, Kotlin, TypeScript) |
| Change-set with 100 dirty signals, applied on the main thread | the platform mirror applying the change-set (`@Observable`, Compose `State`, the TS store): this host measures the core side and a borrowed decode only |
| Keyed patch on 10,000 items, all three | the list mirror applying a patch (the core half is fixed, Finding 1) |
| Core cold start with 100 KB snapshot restore | dlopen/app launch on iOS and Android, wasm compile and instantiate on web (the web row is "after wasm compile") |
| Hello-world size added to the app | release builds for `aarch64-apple-ios`, the Android ABIs and `wasm32-unknown-unknown` (none of these targets is installed here); the host proxy above is thin against 900 KB |
| Runtime memory at idle | a device memory profile (Instruments, Android Studio); the host proxy above is an RSS delta, and an exact heap counter needs a custom global allocator, which is `unsafe` and outside `undra-ffi` (R2) |
| Incremental core rebuild in `undra dev`, 20k-line core | a 20k-line core, which the playground does not yet have |
| Web crash recovery, 1 MB | the wasm build; the restore itself is measured above |
| Comparison with UniFFI and KMP baselines | the playground phase; the blueprint publishes these per release |
