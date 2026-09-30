# Harsh-conditions benchmark: design and implementation brief

**Date:** 2026-09-30 · **Author:** principal architect (design pass) · **Branch:** `wt/stress` ·
**Status:** design, ready for implementation after the integrator's decisions in section 10 ·
**Implements:** launch-v2 design section 5 · **Companion:** `.10x/adrs/ADR-031-frame-coalesced-delivery.md`
(proposed)

The founder asked for "another, more aggressive benchmark that pushes the limit with very, very
high-frequency data, to convince very demanding apps that it survives harsh conditions", with the numbers on
the landing page. This document answers the five design questions from the code (every claim has a
`file:line`), reports what throwaway probes measured, and gives an implementer everything needed to build it.

Contents: 1 the answer in short · 2 what happens at 100 k transactions/s (question 1) · 3 what was measured and
how · 4 scenarios (question 2) · 5 metrics and how to measure them portably (question 3) · 6 budgets (question 4)
· 7 harness architecture · 8 the playground stress screen (question 5) · 9 implementation brief · 10 decisions
for the integrator · 11 findings outside this piece.

---

## 1. The answer in short

* **The core is not the problem.** One observed single-signal transaction, committed and delivered to the
  host, costs 82 ns on the reference host: 12 M transactions/s on one core. Keyed churn on a 10,000-row list
  runs at 280 k ops/s, a 1%-dirty commit over 100,000 observed signals at 36-100 us, a stream holds at most one
  item beyond the consumer's credit at 29 M items/s, and 8 threads completing port calls under a live "main
  thread" lose and reorder nothing.
* **The platform side is.** Nothing between the core and the UI coalesces across transactions: every
  change-set is copied, decoded and applied on the main thread, into queues with no bound, and a keyed patch
  copies the whole list on TypeScript and Kotlin. In TS worker mode each change-set is its own `postMessage`.
  That is a runtime-model gap against the blueprint's own promise ("change-sets committed before the
  platform's next main-thread hop are merged"), so it is written up as **ADR-031 (proposed)**:
  frame-aligned drains, a byte-level merge per signal, a bounded backlog, a batching worker; no wire, ABI or
  core change.
* **The benchmark** is two layers in `bench/` (per-operation p50 rows in the existing gate, and sustained
  scenarios with throughput floors, tail ceilings, invariants and RSS), a soak binary, and a stress screen in
  the web playground whose numbers the landing page shows live. Section 9 is the brief.

## 2. What reaches the platform at 100 k transactions/s (question 1)

### 2.1 In the core: one change-set per transaction per store, delivered synchronously

* Every write outside `ctx.txn` is its own transaction (SPEC 5.5). The runtime does not wrap a dispatched
  call in a transaction (the only `keel_signals::txn` calls in `crates/keel-runtime/src/runtime.rs` are
  `observe` at 1485 and `restore` at 2201), so a method that writes a signal 1,000 times outside `ctx.txn`
  commits 1,000 transactions.
* The commit groups the thread's dirty slots by store and, "for each store, the slots that are observed (or
  `no_coalesce`) are encoded into **one** change-set and handed to the sink" (`crates/keel-signals/src/txn.rs:8-10`;
  `commit` 227-247, `commit_stores` 310-327).
* `StoreCell::commit_slots` takes the store's delivery lock (`crates/keel-signals/src/store.rs:659`), claims
  the dirty observed slots (673-692), builds the payload and calls `sink.deliver` (741) before releasing it.
* The runtime's sink forwards every payload to the host: `RuntimeSink::deliver`
  (`crates/keel-runtime/src/runtime.rs:136-143`) → `deliver_change_set` (846-866) → `Host::change_set`.
* The FFI shells make one foreign call per change-set, on the committing thread: the C callback
  (`crates/keel-ffi/src/native.rs:142-146`), one JNI up-call (`jni_shim.rs:135-137`), one wasm import call
  (`wasm.rs:127-132`).
* **What the core does merge:** writes to one slot inside one transaction are one entry (the dirty bit,
  `store.rs:673-692`), and a slot that is dirty but unobserved is never encoded (`store.rs:79-80`). That is
  the "coalescing" that `#[keel(no_coalesce)]` opts out of today. Nothing merges across transactions and
  nothing is time- or frame-based.

### 2.2 On the platforms: hops are coalesced, work is not, queues are unbounded

| Runtime | How the main thread is reached | Work per change-set | Queue |
|---|---|---|---|
| Swift | `Mirror.enqueue` schedules a `Task { @MainActor }` only if none is pending (`runtimes/swift/KeelRuntime/Sources/KeelRuntime/Core/Mirror.swift:101-115`); the hop clears the flag and drains (117-123, 67-95) | each payload copied on the committing thread (`InprocTransport.swift:339-348` → `KeelCore.swift:685-687`); on main every change-set decoded and every entry applied (`Mirror.swift:91-93, 125-141`, a lock per entry to find the handler); one `@Observable` mutation per entry | `pending: [[UInt8]]`, no bound (`Mirror.swift:23`) |
| Kotlin | `submit` posts a drain only if none is scheduled (`runtimes/kotlin/keel-runtime/runtime/src/main/kotlin/dev/keel/runtime/Mirror.kt:80-84`); up to 32 batches per hop, then re-posts (118-141, 239) | payload copied off the `ByteBuffer` (`InprocTransport.kt:214-215`); on main a full value superseded later **in the same batch** is skipped (158-186, the only real cross-transaction merge in Keel); patches are never merged and each copies the list (`wire/KeyedPatch.kt:131-132`, called per entry by generated code, `examples/playground/generated/kotlin/.../Stores.kt:1286`) | `ConcurrentLinkedQueue`, no bound (`Mirror.kt:43`) |
| TS `wasm-main` | one `flush()` per microtask checkpoint (`runtimes/ts/@keel/runtime/src/mirror.ts:139-150`, default scheduler `queueMicrotask` 69-73) | copied out of wasm memory (`transport/wasm-main.ts:457-459`), decoded and validated on arrival (`mirror.ts:128-141`), every entry applied (181-187, 197-207), subscribers told once per round (`batch`, `signal.ts:48`); a keyed patch is `list.slice()` + ops (`wire/payloads.ts:880-881`, from generated `_apply`, `examples/playground/generated/ts/src/stores.ts:1175`) | `#queue: ChangeEntry[]`, no bound (`mirror.ts:51`) |
| TS `wasm-worker` | **one `postMessage` per change-set** (`src/worker.ts:65-68, 75-77`), so one main-thread task, one microtask checkpoint and one flush each (`transport/wasm-worker.ts:284-285`, `core.ts:624-626`) | as above, plus a React render per flush (`useSignal` is `useSyncExternalStore`, `react.ts:42`) | as above |

### 2.3 The plain answer

At 100 k transactions/s the core emits 100 k change-sets/s, one per transaction per store, each handed
synchronously through the FFI on the committing thread; there is no coalescing, batching or frame alignment
in the core, the runtime or the FFI. The platforms coalesce only the *hop*: a new hop is scheduled as soon as
the previous one has started (Swift clears its flag before draining, `Mirror.swift:117-123`) or has ended with
work queued (Kotlin, `Mirror.kt:135-140`), so an idle main thread is woken for nearly every change-set
(inferred from the code, not measured), and on TS worker mode
every change-set is a main-thread task: **a firehose does mean on the order of 100 k main-thread wake-ups per
second in the worst realistic cases, and always means 100 k per-change-set copies, decodes and applies per
second, an O(list) copy per keyed patch on TS and Kotlin, and an unbounded backlog when the main thread falls
behind or is suspended.** The fix is a frame-coalesced delivery mode in the platform mirrors:
**ADR-031 (proposed)** chooses platform-side merge over core-side coalescing and producer backpressure, and
keeps every guarantee of ADR-019/020/023/027 on the wire.

## 3. What was measured, and how

Throwaway probes (not committed; kept in the session scratchpad): a release-profile integration test in
`bench/` (the real `#[keel::store]` fixtures, `Runtime::new` with no core thread unless stated, the existing
`CountingHost`), the existing `keel_bench::measure::measure` routine for the per-operation numbers, a counting
global allocator in a `keel-ffi` test (the only crate where one is allowed, R2), and Node 24 running the
TypeScript runtime's own `Mirror`, `Signal`, `applyPatch` and wire encoders from source.

Machine: Apple M5 Pro, 18 cores, macOS 26.5, rustc 1.98.1, release profile (LTO fat, codegen-units 1).
**Shared**: load average 4 to 8 during the runs (other agents building), so the cache-bound numbers moved by
up to 3x between runs; both ends are reported. `Instant` on this host ticks at 41.67 ns (24 MHz), so a single
timed operation's p50 includes one tick of quantisation.

| Probe | Result |
|---|---|
| `Ticker.burst(1000)` (1,000 implicit transactions in one call, observed `Signal<u64>`), harness p50 | 82.4 us per call = **82 ns per transaction**, 12.1 M/s; 1,000 change-sets of 37 bytes |
| `Ticker.set(v)` through `Runtime::call_sync`, harness p50 | 152.9-176.5 ns |
| same, 1,000,000 calls timed one by one | 5.70 M/s; p50 166, p99 209, p999 292 ns; max 21 us |
| Allocations per observed single-signal commit (`call_sync_with`, so the call itself allocates nothing, ADR-028) | **3** (0 when unobserved): `group_by_store`'s `groups` and `ids: vec![id]` (`txn.rs:258, 280`) and `claimed` (`store.rs:675`) |
| Keyed churn on 10,000 rows (random insert/remove/move/update, 20/20/20/40), per op | 280 k ops/s; p50 3.5, p99 4.7, p999 7.1 us; 61 bytes per change-set; harness p50 3.65 ms per 1,000 ops |
| Fan-out, one `StoreCell`, 1,000 dirty per transaction, 100,000 observed | 36-103 us per transaction (p99 83-245 us), 21,012 bytes, one change-set |
| same, 10,000 observed | 26-36 us per transaction, 21,012 bytes: cost follows the dirty count; the 100 k case pays cache misses, not an O(observed) step |
| same, 100 dirty of 100,000 | 3.0 us, 2,112 bytes |
| Fan-out over 1,000 stores x 100 signals, one dirty signal in every store | 115-177 us per transaction: 1,000 change-sets (about 115 ns each), 33 KB |
| Stream, always-ready producer, consumer granting 16 at a time (10,000 rounds) | produced − delivered never above **1**; RSS +0 KB |
| Stream, consumer granting 100,000 at a time | 29.0 M items/s (harness: 37 ns per item) |
| Concurrent completions: 8 threads answering async port calls, `keel-core` thread, a thread draining change-sets every 16.7 ms, 256 calls in flight, 400,000 calls | 280 k completions/s; call→reply p50 155 us, p99 1.08 ms; 400,000 change-sets, 0 out of order, 0 lost |
| 12 s mixed loop (firehose bursts + churn), RSS by `ps` each second | 11,296 KB at 1 s, then 11,344 KB flat for 11 samples; 7.3 M change-sets |
| Rust model of a platform mirror (copy + queue on the producer, drain at 60 Hz) at 100 k/s | drain 52 us/frame p50 (applying every entry) vs 35 us merged; at 1 M/s 631 us vs 324 us; queue 3,332 / 33,332 change-sets per frame (a model: Rust applies a `u64` in ~20 ns, platforms pay far more) |
| TS mirror, full-value change-sets (V8) | **~175 ns per change-set** at any batch size (1,667 per flush: 286 us p50, 618 us p99); 226 ns when each change-set gets its own flush |
| TS keyed patch on 10,000 rows | **1.38 us per patch** (the `slice`) however small; 1,667 patches per frame 2.3 ms p50; the same 1,667 ops applied to one copy 5 us |
| Node `worker_threads` `postMessage`, 49-byte envelope, transferred | 1.25 us per message on the receiving thread before any apply; 0.36 us each when 100 travel in one message |

## 4. Scenarios (question 2)

All six proposed scenarios are meaningful and kept, two are sharpened, and two are added. What each proves is
the claim a demanding app team would otherwise ask about.

| # | Scenario | What it proves | Metrics | Proposed CI budget | Measured now (host) |
|---|---|---|---|---|---|
| a | **Firehose**: one observed `Signal<u64>`, one transaction per update, sustained, core-driven (`burst`) and host-driven (`call_sync`) | each transaction costs O(1) in the core at 100x any UI rate; exactly one 37-byte change-set per transaction | txn/s, p50/p99/p999 per txn, bytes/txn, allocations/txn | row A `stress/firehose/txn_x1000` ≤ 420 us, `stress/firehose/call_set` ≤ 770 ns; B ≥ 1.1 M txn/s, p99 ≤ 1.1 us, p999 ≤ 3 us, 37 B/txn exact | 82 ns core-side (12.1 M/s); 5.70 M/s via `call_sync`, p50/p99/p999 166/209/292 ns; 37 B; 3 allocs |
| a' | **Firehose on the web** (playground stress screen, wasm-main) | what the UI thread pays per change-set, before and after ADR-031 | change-sets/s, apply per frame p50/p99, ns per change-set, dropped frames | not CI-gated (device-class numbers); recorded in RESULTS.md and shown live | V8 mirror ~175 ns per change-set; worker mode +1.25 us per message |
| b | **Keyed churn**: 10,000 rows, a fixed cycle of 10 ops (4 update, 2 insert, 2 remove, 2 move) at seeded random positions, one op per transaction, with a host-side list applying every patch | recorded list ops stay O(change) under sustained churn, and the mirror never desynchronises | ops/s, p50/p99/p999 per op (commit + deliver + host apply), bytes/op, final equality | A `stress/keyed_churn_10k/ops_x1000` ≤ 19 ms; B ≥ 56 k ops/s, p99 ≤ 24 us, p999 ≤ 71 us, bytes/op ≤ 1.1x measured, host list == core list | 280 k ops/s, 3.5/4.7/7.1 us, 61 B/op (random mix; re-measure with the fixed cycle) |
| c | **Fan-out**: 100,000 observed signals in one store cell, 1% (1,000) dirty per transaction, and the same 1,000 dirty over 10,000 observed | commit time and bytes follow the dirty count, not the observed count | txn/s, p99 per txn, bytes/txn, time ratio 100 k / 10 k | A `stress/fanout/100k_observed_1k_dirty` ≤ 500 us, `.../10k_observed_1k_dirty` ≤ 180 us; B ≥ 2,500 txn/s, p99 ≤ 1.3 ms, 21,012 B exact, ratio ≤ 4, bytes equal | 36-103 us vs 26-36 us; 21,012 B both |
| c' | **Fan-out across stores** (added): 1,000 stores, one dirty signal in each, one transaction | the per-store overhead when one transaction touches many stores (1,000 change-sets sharing a `txn_id`), the worst case for ADR-031's merge | txn/s, change-sets/txn | A `stress/fanout/1k_stores_1k_dirty` ≤ 890 us (re-baseline) | 115-177 us, 1,000 change-sets |
| d | **Stream backpressure**: an always-ready producer (so its rate is whatever the core polls, far above 1 M items/s), a consumer granting 16 credits per round; then a consumer granting 100,000 | Keel buffers at most one item beyond credit, so memory is bounded whatever the producer does; the item path is fast when credit allows | max(produced − delivered), RSS growth, items/s | A `stress/stream/items_x1000` ≤ 190 us; B max ahead ≤ 1 exact, RSS growth ≤ 1%, ≥ 5.7 M items/s | 1; +0 KB; 29 M items/s |
| e | **Concurrent completions**: 8 host threads answering async port calls, a `keel-core` thread, a "main" thread draining change-sets at 60 Hz, 256 calls in flight | the core lock and the per-store delivery lock keep order under contention: nothing lost, nothing reordered | completions/s, call→reply p50/p99, lost, out-of-order `txn_id`s, final sum | B ≥ 56 k/s, p99 ≤ 5.4 ms, lost = 0, out-of-order = 0, sum exact | 280 k/s, 155 us / 1.08 ms, 0, 0 |
| f | **Soak**: the mixed, paced load of a busy app (firehose 100 k txn/s, churn 20 k ops/s, completions 50 k/s, a stream at 1 M items/s, a 60 Hz drain) for 60 s locally, 10 s in CI | no leak, no drift | RSS growth after warm-up, p99 per 1 s window, achieved rates | RSS ≤ +1% (or ≤ 64 KiB) from the first post-warm-up sample to the last; worst window p99 ≤ 3x the median window p99; invariants of a, b, e | 12 s probe: 0.0% after the first second |
| g | **Event firehose** (added): `Runtime::event` on an event port whose subscriber writes a signal | the path a WebSocket or sensor feed takes into the core; `event` takes the core lock on the caller's thread (`runtime.rs:1975-1990`), which is natural backpressure for a network thread | events/s, p50 per event | A `stress/firehose/event` ≤ 5x baseline | not measured: baseline in the piece |

Dropped or reshaped, with the reason:

* "(c) 100 k observed signals" cannot be a `#[keel::store]` (one field per signal); it is measured at the
  `StoreCell` level with `keel-signals`' public API, as `signals/changeset_100/cell` already is. The
  many-stores variant (c') is the realistic app shape and goes through the same public API.
* "(d) a producer at 1 M items/s" is reshaped: Keel's stream plumbing pulls (`drive_stream` polls one item,
  then waits for credit, `runtime.rs:2351-2384`; credit accounting 255-277), so a producer's rate is whatever
  the core polls. The claim worth proving is "Keel never holds more than one item beyond credit", with a
  producer that is always ready (faster than any push source). A push source inside an app (a channel fed by
  a port) is the app's buffer and must be bounded by the app; the docs page for ADR-031 says so. The soak runs
  a stream at 1 M items/s with a consumer that keeps up.
* The web side (a') is not CI-gated: a shared runner's headless Chromium says nothing about a user's device.

## 5. Metrics, and how to measure them portably (question 3)

* **Sustained throughput**: operations completed divided by the wall time of the whole run (not the sum of
  per-operation samples). Runs are 2 s per scenario in CI (`KEEL_STRESS_SECONDS`), 10 s locally for the
  RESULTS.md numbers.
* **p50/p99/p999**: every operation timed with one `Instant` pair and recorded in a fixed log-linear
  histogram (`keel_bench::stats::Histogram`, section 7.2): no allocation per sample, 3.1% relative error. The
  step timed is named per scenario: firehose, one `call_sync` (commit and host callback included); churn, one
  op including the host-side patch apply; fan-out, one transaction; completions, `call` to reply. The p50
  includes the clock (one 41.67 ns tick on Apple silicon; vDSO `clock_gettime` on Linux).
* **Change-set bytes per transaction**: the host counts `payload.len()` (`CountingHost::change_set_bytes`,
  `bench/common/host.rs:53-57`). Deterministic for a given seed, so gated exactly or at 1.1x.
* **Allocations per operation**: the bench crate cannot count them (`#![forbid(unsafe_code)]`, R2; a global
  allocator is `unsafe impl`). The only counting harness is `crates/keel-ffi/tests/sync_alloc.rs`. Measured
  today: 3 per observed commit. Gate it in `keel-ffi` (decision D4), report it in RESULTS.md.
* **Memory steady state**: resident set size, sampled once a second outside the timed region, by
  `keel_bench::rss::resident_bytes()` with **std only** (no new dependency, nothing that has to build for
  wasm, iOS or Android: the bench crate is host-only and `publish = false`):
  * Linux (and Android, should it ever run there): parse `VmRSS:` (kB) from `/proc/self/status`.
  * macOS and other Unix: `ps -o rss= -p <pid>` (kB) through `std::process::Command`, the same measure
    RESULTS.md already uses for "Runtime memory at idle"; about 2-5 ms per call, so at most 1 Hz.
  * Anything else: `None`, and the RSS gates are skipped with a printed notice (never a silent pass).
  RSS is page-granular (16 KiB pages on Apple silicon), so a gate is "growth ≤ 1% **or** ≤ 64 KiB", measured
  from the first sample after a warm-up (20% of the run) to the last sample.
* **Queue depth / backlog** (soak and scenario e): the largest number of change-sets a drain found.

## 6. Budgets (question 4)

The rules are the existing file's. Layer A rows (`[bench."stress/..."]`, p50 per iteration) use exactly the
existing rule: 5x the measured p50, floor 250 ns, rounded up to two significant figures. Layer B rows
(`[stress."..."]`, new table kind, section 7.2) use: throughput floors at measured / 5 rounded **down** to two
significant figures; p99 ceilings at 5x and p999 ceilings at 10x (tails on a shared runner are noisier than
medians); bytes exact when fully deterministic, else 1.1x; RSS growth 1%. `KEEL_BENCH_SCALE` divides floors
and multiplies ceilings; it never touches bytes, RSS or invariants. Invariants (exact equalities, zero lost,
zero out-of-order, max-ahead ≤ 1) live in the code, not the file.

"Measured" values below come from the probes of section 3. Where marked *re-baseline*, the implementer replaces
them with the harness's own `baseline` output on a quiet machine before merging (the fixed churn cycle and the
cache-bound fan-out rows especially).

```toml
# ---------------------------------------------------------------------------------------------
# Harsh conditions, layer A: the unit of work of each stress scenario, gated like every row above
# (p50 per iteration, 5x, floor 250 ns). Design: .10x/specs/2026-09-30-stress-bench-design.md.
# ---------------------------------------------------------------------------------------------

# 1,000 implicit transactions on one observed Signal<u64> in one call (a core-side producer):
# 1,000 change-sets of 37 bytes. 82 ns per transaction on the reference host.
[bench."stress/firehose/txn_x1000"]
budget_ns = 420000
measured_ns = 82350.7

# One `Ticker.set(v)` through `Runtime::call_sync`: a host pushing updates one call at a time.
[bench."stress/firehose/call_set"]
budget_ns = 770
measured_ns = 152.9

# One event through `Runtime::event` whose subscriber writes an observed signal. Re-baseline.
[bench."stress/firehose/event"]
budget_ns = 1000   # placeholder: set from `baseline`, 5x rule
measured_ns = 200  # placeholder

# 1,000 recorded list ops (4 update, 2 insert, 2 remove, 2 move per 10) on a 10,000-row keyed list.
# Measured with a random 20/20/20/40 mix; re-baseline with the fixed cycle.
[bench."stress/keyed_churn_10k/ops_x1000"]
budget_ns = 19000000
measured_ns = 3653708.0

# One transaction writing 1,000 of 100,000 observed signals of one store cell: one change-set of
# 21,012 bytes. Cache-bound: 36-103 us across runs on a loaded host. Re-baseline on a quiet machine.
[bench."stress/fanout/100k_observed_1k_dirty"]
budget_ns = 500000
measured_ns = 98477.3

# The same 1,000 dirty signals over 10,000 observed: the pair shows cost follows the dirty count.
[bench."stress/fanout/10k_observed_1k_dirty"]
budget_ns = 180000
measured_ns = 36000    # probe, not the harness: re-baseline

# One transaction writing one signal in each of 1,000 stores: 1,000 change-sets.
[bench."stress/fanout/1k_stores_1k_dirty"]
budget_ns = 890000
measured_ns = 176541   # probe, not the harness: re-baseline

# 1,000 stream items: grant 1,000 credits, run the executor until idle.
[bench."stress/stream/items_x1000"]
budget_ns = 190000
measured_ns = 36986.7

# ---------------------------------------------------------------------------------------------
# Harsh conditions, layer B: sustained runs (`cargo test -p keel-bench --test stress --release`).
# Floors = measured / 5 rounded down (2 s.f.); p99 = 5x, p999 = 10x measured, rounded up; bytes
# exact or 1.1x; RSS growth in percent. Invariants are asserted in code.
# ---------------------------------------------------------------------------------------------

[stress."firehose/sustained"]
min_per_sec = 1100000
p99_ns = 1100
p999_ns = 3000
bytes_per_op = 37
measured_per_sec = 5699209
measured_p99_ns = 209
measured_p999_ns = 292
measured_bytes_per_op = 37

[stress."keyed_churn_10k/sustained"]
min_per_sec = 56000
p99_ns = 24000
p999_ns = 71000
bytes_per_op = 68          # 1.1x the measured 61.0 (random mix); re-baseline with the fixed cycle
measured_per_sec = 280426
measured_p99_ns = 4694
measured_p999_ns = 7064
measured_bytes_per_op = 61.0

[stress."fanout/sustained"]
min_per_sec = 2500
p99_ns = 1300000           # 5x the loaded run (244,458); the quiet run measured 82,709
bytes_per_op = 21012
measured_per_sec = 12689
measured_p99_ns = 244458
measured_bytes_per_op = 21012

[stress."stream/backpressure"]
min_per_sec = 5700000      # the fast-consumer phase, items/s
rss_growth_pct = 1
measured_per_sec = 28996862

[stress."completions/8_threads"]
min_per_sec = 56000
p99_ns = 5400000
measured_per_sec = 280321
measured_p99_ns = 1079000

[stress."soak/mixed"]
rss_growth_pct = 1
```

## 7. Harness architecture

### 7.1 Two layers, one set of fixtures

* **Layer A (per-operation, existing gate).** The unit of work of each scenario is a `Workload` in a new
  `stress` group (`bench/common/stress.rs::workloads()`), added to `workloads::all()` and `group("stress")`,
  so the existing `budgets` test, the `baseline` printer and criterion (`bench/benches/stress.rs`) all get it
  with no change to their code. It catches "an operation got several times slower".
* **Layer B (sustained).** Each scenario runs for a fixed wall time with its producers, consumers and
  threads, records a latency histogram, counts, bytes and RSS, and checks its invariants:
  `bench/common/stress.rs::scenarios()`, run by the new `bench/tests/stress.rs` against `[stress."..."]`
  budgets. It catches "it is fast once but not for two seconds", tails, leaks, reordering and loss.
* **Soak.** `bench/src/bin/soak.rs` runs the mixed paced load of scenario f for N seconds and exits non-zero
  on RSS growth or p99 drift. It includes `common/fixtures.rs`, `common/host.rs` and `common/stress.rs` by
  `#[path]` (not `common/mod.rs`, which uses criterion, a dev-dependency a binary cannot use); those files
  refer to each other through `super::`, which resolves to the crate root in the binary and to `common` in
  the tests, so both inclusions work.

### 7.2 Library additions (`bench/src/`, no fixtures, `#![forbid(unsafe_code)]`, documented, unit-tested)

```rust
// bench/src/stats.rs
/// Latency histogram: 64 linear 1 ns buckets below 64 ns, then 32 sub-buckets per power of two up to
/// 2^40 ns; 1,184 u64 counters in one boxed array; relative error <= 1/32.
pub struct Histogram { /* counts: Box<[u64; BUCKETS]>, total: u64, max: u64 */ }
impl Histogram {
    pub fn new() -> Histogram;
    pub fn record(&mut self, ns: u64);            // no allocation
    pub fn count(&self) -> u64;
    pub fn max(&self) -> u64;
    pub fn percentile(&self, p: f64) -> u64;      // upper bound of the bucket holding the p-quantile
    pub fn merge(&mut self, other: &Histogram);
    pub fn clear(&mut self);
}

// bench/src/rss.rs
/// Resident set size of this process in bytes: /proc/self/status (Linux), `ps -o rss=` (macOS, other
/// Unix), None elsewhere. Page-granular; costs milliseconds on macOS, so sample at most once a second.
pub fn resident_bytes() -> Option<u64>;
/// RSS samples over a run, and the growth check the soak and the stream scenario use.
pub struct RssSeries { /* samples: Vec<(Duration, u64)> */ }
impl RssSeries {
    pub fn new() -> RssSeries;
    pub fn sample(&mut self, at: Duration);                     // no-op when resident_bytes() is None
    pub fn growth(&self, warmup: f64) -> Option<RssGrowth>;     // first sample at or after warmup*run .. last
}
pub struct RssGrowth { pub baseline_bytes: u64, pub final_bytes: u64, pub peak_bytes: u64, pub growth_pct: f64 }
impl RssGrowth { pub fn within(&self, pct: f64) -> bool /* growth_pct <= pct || final - baseline <= 64 KiB */ }

// bench/src/budget.rs (extended)
pub struct StressBudget {
    pub min_per_sec: Option<f64>, pub p99_ns: Option<f64>, pub p999_ns: Option<f64>,
    pub bytes_per_op: Option<f64>, pub rss_growth_pct: Option<f64>,
    pub measured_per_sec: Option<f64>, pub measured_p99_ns: Option<f64>,
    pub measured_p999_ns: Option<f64>, pub measured_bytes_per_op: Option<f64>,
}
pub struct Budgets { pub meta: .., pub benches: .., pub stress: BTreeMap<String, StressBudget> }
// New table header `[stress."name"]`; keys above only (anything else is a Syntax error with the line);
// a stress table with none of min_per_sec, p99_ns, p999_ns, bytes_per_op, rss_growth_pct is
// BudgetError::MissingGate { name }; a repeated name is Duplicate. Existing tests keep passing.
```

### 7.3 Fixtures and hosts (`bench/common/`)

* `fixtures.rs` gains, with the same doc style:
  * `Ticker` store: `value: Signal<u64>`; `new()`, `set(v: u64)`, `burst(k: u32)` (k implicit transactions).
  * `Churn` store: `#[keel(key = "id")] rows: Signal<Vec<Item>>`, `next_id: AtomicU64`, `rng: Mutex<u64>`;
    `new()`, `seed(count: u32)`, `churn(k: u32)` (k ops, each its own transaction, following the fixed cycle
    `[update, insert, update, move, remove, update, insert, move, update, remove]` at positions from a
    seeded xorshift64; the length is back to its seed after every 10 ops), `ids() -> Vec<u64>`.
  * `Producer` object: `numbers(count: u64) -> impl Stream<Item = u64>` (always ready, counts what it
    produced), `produced() -> u64`.
  * `Fetcher` store: `ctx: Ctx`, `total: Signal<u64>`; `async fn fetch(i: u32) -> u64` awaits
    `ctx.port_call(SOURCE_PORT, 1, i)`, adds the reply to `total`; `total_now() -> u64`.
    `pub const SOURCE_PORT: u32` (a foreign port bound with `Runtime::bind_foreign_port`).
  * `Ticks` event port (`#[keel::port(event)] pub trait Ticks { fn tick(&self, value: u64); }`) and a
    `TickSink` store that subscribes in its constructor (`ctx.events().subscribe(..)`, the `Subscription`
    kept in the store) and writes `value: Signal<u64>` per event. If the subscription API makes this more than
    an hour's work, drop row g and record why in the decision record.
* `host.rs` gains:
  * `CopyingHost`: `CountingHost` plus a copy of every change-set into one reused `Vec<u8>` (the copy each
    platform's FFI callback makes: `InprocTransport.swift:347`, `InprocTransport.kt:215`,
    `wasm-main.ts:458`), without growing memory.
  * `ApplyingHost`: decodes each change-set (`ChangeSetRef`) and applies `op 1` entries for one handle to a
    host-side `Vec<Item>` with `KeyedPatch::<Item>::decode` + `apply` (op 0 replaces it): the Rust stand-in
    for a platform list mirror.
  * `DrainHost`: copies each change-set into a `Mutex<Vec<Vec<u8>>>`; a separate "main" thread (spawned by
    the scenario) takes the vector every 16.667 ms, walks each change-set, checks per-store `txn_id`s strictly
    increase, and records the largest batch. Also answers port calls `Async` and pushes the `port_call_id` onto
    a `Mutex<VecDeque<u32>>` + `Condvar` for the completer threads; counts replies and stream items.

### 7.4 `bench/common/stress.rs`

```rust
pub fn workloads() -> Vec<Workload>;                   // the eight layer-A rows of section 6
pub struct StressConfig { pub duration: Duration, pub rss: bool }
pub struct StressReport {
    pub name: &'static str,           // the [stress."name"] key
    pub ops: u64, pub elapsed: Duration,
    pub latency: Histogram,           // empty where the scenario has no per-op latency (stream)
    pub bytes: u64,                   // change-set (or stream item) bytes over the run
    pub rss: Option<RssGrowth>,
    pub invariants: Vec<Invariant>,   // Invariant { what: String, holds: bool }
    pub notes: Vec<String>,           // e.g. "max queue 3,332 change-sets"
}
impl StressReport { pub fn per_sec(&self) -> f64; pub fn bytes_per_op(&self) -> f64; }
pub fn scenarios() -> Vec<(&'static str, fn(&StressConfig) -> StressReport)>;
pub fn firehose(cfg: &StressConfig) -> StressReport;         // "firehose/sustained"
pub fn keyed_churn(cfg: &StressConfig) -> StressReport;      // "keyed_churn_10k/sustained"
pub fn fanout(cfg: &StressConfig) -> StressReport;           // "fanout/sustained"
pub fn stream_backpressure(cfg: &StressConfig) -> StressReport; // "stream/backpressure"
pub fn completions(cfg: &StressConfig) -> StressReport;      // "completions/8_threads"
```

How each is driven (the probes in section 3 did exactly this):

* **firehose**: `runtime()` with `CopyingHost`; `Ticker` observed; 1,024 prebuilt `set` payloads; loop until
  `duration`: time one `call_sync`, record. Invariants: change-sets == ops; bytes == 37 x ops.
* **keyed_churn**: `runtime()` with `ApplyingHost`; `Churn::seed(10_000)`, observed; loop: time one
  `churn(1)` call (dispatch is ~150 ns of a ~3.5 us op), record. Invariants: after the run the host list's ids
  equal `Churn::ids()`; length 10,000 after a multiple of 10 ops; no warnings logged.
* **fanout**: `drive_from_this_thread()`; one `StoreCell` with 100,000 attached `Signal<u32>`, observed;
  a `with_sink(CountingSink)` transaction writing 1,000 signals at stride 100 with a rotating offset; timed per
  transaction. For the last quarter of the run the same with 10,000 observed. Invariants: bytes per
  transaction 21,012 in both; p50(100 k) ≤ 4 x p50(10 k).
* **stream_backpressure**: `Runtime::new` (no core thread) with a counting host; `Producer.numbers(u64::MAX)`
  called with `call`; half the time: `stream_credit(id, 16)` + `run_pending()`, check
  `produced() − delivered ≤ 1` every round, RSS sampled each 250 ms; then the other half with 100,000 per grant
  for items/s. Invariants: max ahead ≤ 1; RSS within 1%.
* **completions**: `Runtime::new` with `core_threads: 1` and `DrainHost`; `bind_foreign_port(SOURCE_PORT)`;
  `Fetcher` observed; 8 completer threads pop ids and call `port_reply(port_reply_ok(id, 1u64))`; the issuer
  keeps 256 `fetch` calls in flight with `call` and records call→reply in the histogram (the reply callback
  looks up the issue time); the drain thread checks order. Invariants: replies == issued, out-of-order == 0,
  `total_now()` == issued. Shut everything down (`Runtime::shutdown`) at the end.

### 7.5 `bench/tests/stress.rs`

* `stress_table_covers_every_scenario`: `[stress."..."]` names == `scenarios()` names plus `soak/mixed`
  (read by the soak binary); every measured value within its own gate.
* `stress`: serial (a `static SERIAL: Mutex<()>`, as `budgets.rs`); release: each scenario for
  `KEEL_STRESS_SECONDS` (default 2) and gated (floors / scale, ceilings x scale, bytes, RSS, invariants);
  debug: each for 100 ms, **invariants only** (so `cargo test --workspace` stays fast and still proves the
  scenarios work). `KEEL_BENCH_FILTER` applies. Prints a table: name, per_sec, p50, p99, p999, bytes/op, RSS
  growth, verdict.
* `stress_baseline` (`#[ignore]`): prints `[stress."..."]` tables with the section 6 rules.
* `KEEL_STRESS_JSON=<path>`: also writes the reports as JSON rows (section 7.8), for the site.

### 7.6 `bench/src/bin/soak.rs`

* Arguments (parsed by hand from `std::env::args`, no clap): `--seconds N` (default 60), `--warmup PCT`
  (default 20), `--json PATH`, `--rss-limit-pct X` (default: `rss_growth_pct` of `[stress."soak/mixed"]` in
  `bench/budgets.toml`, read with `Budgets::load`). `#![forbid(unsafe_code)]`.
* One `Runtime` (`core_threads: 1`) with a `DrainHost`-style host; `Ticker`, `Churn` (10,000 rows),
  `Fetcher` and a `Producer` stream, all observed. Paced producers, each on its own thread, in 1 ms slices
  (sleep the rest of the slice): firehose 100 `set` per ms (100 k/s); churn `churn(20)` per ms (20 k/s);
  completions 50 per ms with a window of 64 (50 k/s, 8 completer threads); stream credit 1,000 per ms
  (1 M items/s). The drain thread at 60 Hz as in scenario e. Fan-out is not in the soak: it runs at the
  `StoreCell` level without a runtime, and scenario c already covers it.
* Every second: RSS sample, the firehose `call_sync` p99 of that second (a per-second `Histogram`), achieved
  rates, largest drain batch; one line to stdout (and to `--json` as an array).
* Exit 1 when: RSS growth from the first post-warm-up sample to the last > limit (and > 64 KiB); the worst
  post-warm-up window's p99 > 3x the median window p99; any invariant broken (out-of-order, lost
  completion, host list ≠ core list at the end). A window whose achieved rate is under 50% of its target
  also fails (the host could not carry the load); under 95% prints a warning.
* CI runs `--seconds 10`; RESULTS.md numbers come from `--seconds 60`.

### 7.7 CI (`.github/workflows/bench.yml`)

Two steps after the existing one, same job:
`cargo test -p keel-bench --test stress --release -- --nocapture` and
`cargo run -p keel-bench --release --bin soak -- --seconds 10`. Budget about 1 minute extra.

### 7.8 RESULTS.md and `site/data/bench.json`

* `bench/RESULTS.md` gains **"Harsh conditions"** after "Section 14 rows": one table (scenario, what it
  proves, measured on the reference host from `--seconds 60` / `KEEL_STRESS_SECONDS=10`, CI gate, verdict),
  the in-browser numbers from the stress screen with the browser, device and date, and a short honesty
  paragraph: host numbers are the core side; the platform apply is measured in the browser and, for iOS and
  Android, in the device phase; what ADR-031 changes once implemented (before and after on the same screen).
  "The CI gate" section mentions the second test and the soak.
* `site/data/bench.json` is owned by site-v2 (it lands first). Stress adds rows to its "harsh" group in
  whatever shape site-v2 chose; the proposed row, which `KEEL_STRESS_JSON` and `soak --json` emit:

```json
{
  "group": "harsh",
  "id": "stress/firehose",
  "label": "Firehose: one signal, one transaction per update",
  "value": "5.7 M txn/s",
  "detail": "p99 209 ns per transaction (commit + deliver), 37 bytes each",
  "gate": "CI: at least 1.1 M txn/s, p99 at most 1.1 us",
  "where": "Apple M5 Pro, core only (host)",
  "source": "bench/RESULTS.md#harsh-conditions"
}
```

  Landing-page lines this supports, each with "host, core side" attached: 12 M transactions/s on one core
  (82 ns each); 280 k list operations/s on a 10,000-row list at 61 bytes each; 1,000 of 100,000 observed
  signals changed in one 21 KB change-set; a stream never more than one item ahead of its consumer at 29 M
  items/s; 8 threads x 280 k completions/s with nothing lost or reordered; RSS flat over the soak. Plus the
  live number from the visitor's own browser.

## 8. The playground stress screen (question 5)

### 8.1 Core-generated, Timer-paced, Clock-compensated (not a host loop)

Decision: the updates are **generated in the core by a task paced by the `Timer` port**, not by a host loop
calling methods. A host loop would deliver every change-set inside a call, before its reply, which is the
read-your-writes path (ADR-031 decision 2) and not the firehose; real high-frequency data (a socket, a
sensor, a simulation) arrives in the core on its own. A synchronous `burst` method also exists for the Rust
benches, the unit tests and contract scenario S18.

`examples/playground/core/src/stress.rs` (new module, `pub use stress::Stress` in `lib.rs`, a row in the
module table of `lib.rs`'s docs):

```rust
/// Which kind of update the generator makes.
#[keel::api] #[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StressMode { Firehose, Churn, Board }

/// What `Stress::start` runs.
#[keel::api] #[derive(Clone, Debug, PartialEq)]
pub struct StressConfig { pub mode: StressMode, pub rate_per_sec: u32, pub seed: u64 }

/// One cell of the ticker board.
#[keel::api] #[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quote { pub id: u32, pub cents: i64, pub change: i32 }

/// Why a start was refused.
#[keel::error] #[derive(Clone, Debug, PartialEq, Eq)]
pub enum StressError { #[error("rate {rate} is outside 1..=2000000 transactions per second")] RateOutOfRange { rate: u32 } }

#[keel::store(restore = "Self::assemble")]
pub struct Stress {
    ctx: Ctx,
    generator: Arc<Mutex<Generator>>,        // config, rng state, epoch, carry; private
    value: Signal<u64>,                      // Firehose: +1 per transaction
    #[keel(key = "id")] rows: Signal<Vec<Item>>,   // Churn: 10,000 rows, one recorded op per transaction
    #[keel(key = "id")] board: Signal<Vec<Quote>>, // Board: 1,000 quotes, 10 update_at per transaction (1%)
    generated: Signal<u64>,                  // transactions since start; written once per tick
    running: Signal<bool>,
}
// #[keel::api(store)] impl Stress:
pub fn new(ctx: Ctx) -> Self;
pub fn start(&self, config: StressConfig) -> Result<(), StressError>; // starts, or retunes a running generator
pub fn stop(&self);                                                   // the task ends at its next tick
pub fn burst(&self, mode: StressMode, transactions: u32);             // now, synchronously, deterministic
pub fn reset(&self);                                                  // one transaction: every signal back to its start
```

The generator task (spawned by `start` with `self.ctx.spawn`, holding clones of the signals and the
`Arc<Mutex<Generator>>`, never `self`):

```text
last = clock(ctx).monotonic_ns()
loop:
    ctx.sleep(16 ms).await                                 # Timer port: setTimeout on web
    lock generator; if generator.epoch != my_epoch: break  # stop() or a later start() bumped it
    now = clock(ctx).monotonic_ns()
    acc += rate * (now - last); last = now                 # integer ns x rate, no floats
    due = min(acc / 1e9, rate / 10); acc %= 1e9            # at most 100 ms of work per tick (hidden tab)
    run `due` transactions of the mode:
        Firehose: value.update(+1)                                          (one implicit txn each)
        Churn:    the fixed 10-op cycle of section 7.3 at xorshift64 positions (one recorded op each)
        Board:    ctx.txn(|| 10 x board.update_at(pos, |q| { q.cents += delta; q.change = delta }))
    generated.update(+due)                                 # one extra change-set per tick
```

Determinism (R12): no wall clock (the `Clock` port), no ambient randomness (xorshift64 seeded from
`StressConfig::seed`; the `Rng` port is deliberately not used, so a run is reproducible), no thread (a task on
the core's executor, timers through the `Timer` port). The sequence of committed states is a pure function of
`(seed, mode, the successive due counts)`; under `TestRuntime` and `FakeClock` it is exact, and the unit tests
assert exact final values. Generation speed on a real device depends on the timer, which is the point: the
screen reports what was achieved, not what was asked.

Unit tests (in `stress.rs`, `TestRuntime`): `burst` of each mode is deterministic for a seed (exact final
`value`, row ids, board checksum); `start(rate 1000)` then `advance(16 ms)` x 62 commits exactly 992
transactions and `generated == 992`; `stop` ends the task (no change after further `advance`); `start` while
running retunes without a second task; `rate 0` and `2_000_001` are refused; the churn list is 10,000 rows
after every multiple of 10 ops; a snapshot/restore round trip keeps the rows; the board patch of one Board
transaction is one change-set with a 10-op keyed patch.

### 8.2 The web screen

Files: `examples/playground/web/src/views/StressView.tsx` (new), `examples/playground/web/src/stress-stats.ts`
(new: `WindowStats`, `FrameMonitor`, `percentile`), `App.tsx` (a `stress` tab; `screen=stress` from site-v2's
URL parameters), `keel.ts` unchanged (the view creates the store with `useKeel(Stress)` so the 11,000 rows
exist only while the tab is open), `index.css` (tiles, grid). The generated bindings come from
`keel bindgen` (section 9).

What it shows:

* Controls: mode (Firehose · Keyed churn 10k · Ticker board 1k), rate (1 k, 10 k, 100 k, 250 k, 1 M per
  second), Start/Stop; URL parameters `mode=`, `rate=`, `autostart=1` (in addition to site-v2's `screen=`,
  `embed=`).
* Six tiles, refreshed every 500 ms: **Generated** (transactions/s: Δ`generated` / Δt), **Received**
  (change-sets/s: Δ`KeelCore.shared.mirror.changeSets` / Δt), **Applied** (entries/s, from the drain listener;
  equal to received entries until ADR-031 merges), **Apply per frame** (p50 / p99 of drain durations, us),
  **Apply per change-set** (Σ drain time / Σ change-sets, ns: the average survives timer quantisation),
  **Dropped frames** (last 5 s / since start) and the longest frame (ms).
* The visual: Firehose, `value` as a large tabular number and a 60-point sparkline of received/s; Churn, the
  virtualised 10,000-row window (reuse `BigListView`'s row height and overscan) with changed rows flashing;
  Board, a 40 x 25 grid of quotes coloured by the sign of `change`.
* One sentence under the tiles, verbatim: "Measured in your browser. The core is Rust compiled to
  WebAssembly, running on this page's main thread. Each transaction writes one signal (firehose), one list
  row (churn) or 10 of 1,000 quotes (board)."

How it measures:

* **Apply**: a drain listener on the TS mirror, `mirror.addDrainListener(fn): () => void`, called after each
  flush with `{ changeSets, entries, appliedEntries, durationMs }`, timed inside the runtime with
  `performance.now()` only while a listener is registered (decision D2; the fallback without the runtime
  change is wrapping `KeelCore.shared.mirror.flush` from the view, which works because the scheduled flush
  calls `this.flush()`). `performance.now()` is coarsened by browsers (Chrome: 100 us without cross-origin
  isolation, which GitHub Pages cannot enable; 5 us with it), so per-drain percentiles are quantised; the
  per-change-set mean is the robust number, and the screen shows the resolution it detected.
* **Dropped frames**: `FrameMonitor` runs a `requestAnimationFrame` loop; the nominal interval is the median
  of the first 60 gaps after start (handles 60/90/120 Hz); a gap over 1.5x nominal drops
  `round(gap / nominal) − 1` frames. Paused while `document.hidden`.
* **Stats protocol**: in embed mode, every 500 ms, `window.parent.postMessage(stats, targetOrigin)` with
  site-v2's base shape plus optional fields (a consumer that knows only the base keeps working):

```ts
interface KeelStats {
  type: "keel-stats";
  changeSetsPerSec: number;            // site-v2 base: received by the mirror
  applyP50Us: number;                  // site-v2 base: per drain (per change-set when one drain = one change-set)
  applyP99Us: number;                  // site-v2 base
  generatedPerSec?: number;            // stress: transactions the core committed
  entriesAppliedPerSec?: number;       // stress: after merging (ADR-031)
  drainsPerSec?: number;
  applyNsPerChangeSet?: number;        // Σ drain time / Σ change-sets over the window
  droppedFrames?: number;              // in this window
  longestFrameMs?: number;
  mode?: "firehose" | "churn" | "board";
  targetRate?: number;
  timerResolutionUs?: number;          // the smallest non-zero performance.now() step seen
  runtime?: "wasm-main" | "wasm-worker";
}
```

  `targetOrigin` is the site's origin when the page knows it (`document.referrer`), else `"*"` (the message
  carries no user data). The landing page's "Push it" control loads `playground/?screen=stress&embed=1&
  mode=firehose&rate=100000&autostart=1`.

### 8.3 iOS and Android (device phase, not in this piece)

The same `Stress` store; `StressView.swift` and `StressScreen.kt` with the same tiles; apply measured by the
Swift/Kotlin drain listeners of ADR-031 decision 5 with `os_signpost` / `FrameMetrics` for frames. Numbers
recorded on the blueprint's devices only (RESULTS.md's rule: no simulator numbers).

## 9. Implementation brief

### 9.1 Pieces and order

* **S1 `stress` (this branch, implementable now)**: sections 7 and 8 with the TS drain listener (D2). No ADR
  needed: no wire, ABI, threading or generated public shape changes (the playground's own bindings grow a
  store, which is app code).
* **S2 `coalesce` (after ADR-031 is accepted)**: ADR-031 in TS first (it has the worst case, worker mode,
  and the live demo), then Kotlin, then Swift; contract scenario S18; the stress screen re-measured before and
  after, both recorded. Decision 6 of the ADR may be a later S3.

### 9.2 S1 file list

| Action | File | What |
|---|---|---|
| modify | `bench/Cargo.toml` | `[[bench]] name = "stress" harness = false` (keep the comment: every bench target is listed) |
| new | `bench/src/stats.rs` | `Histogram` (7.2) + unit tests (known distributions, merge, max, empty, relative error bound) |
| new | `bench/src/rss.rs` | `resident_bytes`, `RssSeries`, `RssGrowth` (7.2) + unit tests (Some and > 0 on macOS/Linux; growth math on synthetic samples; the 64 KiB floor) |
| modify | `bench/src/lib.rs` | `pub mod rss; pub mod stats;` and a paragraph on layer B in the crate docs |
| modify | `bench/src/budget.rs` | `[stress."name"]` tables, `StressBudget`, `Budgets::stress`, `BudgetError::MissingGate` + tests (parse, unknown key, missing gate, duplicate, the shipped file parses) |
| modify | `bench/common/fixtures.rs` | `Ticker`, `Churn`, `Producer`, `Fetcher`, `SOURCE_PORT`, `Ticks` + `TickSink` (7.3) |
| modify | `bench/common/host.rs` | `CopyingHost`, `ApplyingHost`, `DrainHost` (7.3) |
| new | `bench/common/stress.rs` | `workloads()`, `StressConfig`, `StressReport`, `Invariant`, the five scenario functions, `scenarios()` (7.4); each workload asserts at build time that it does what its name says (bytes, change-set counts, list length), like `workloads.rs` |
| modify | `bench/common/mod.rs` | `pub mod stress;` |
| modify | `bench/common/workloads.rs` | `all()` extends with `super::stress::workloads()`; `group("stress")` |
| new | `bench/benches/stress.rs` | criterion bench over `group("stress")`, the 18-line shape of `benches/signals.rs` |
| new | `bench/tests/stress.rs` | 7.5 |
| new | `bench/src/bin/soak.rs` | 7.6 |
| modify | `bench/budgets.toml` | the section 6 rows, re-baselined where marked; the header comment gains the layer B rule |
| modify | `bench/RESULTS.md` | "Harsh conditions" (7.8); "The CI gate" mentions `--test stress` and the soak |
| modify | `.github/workflows/bench.yml` | 7.7 |
| new | `examples/playground/core/src/stress.rs` | 8.1 with its unit tests |
| modify | `examples/playground/core/src/lib.rs` | `pub mod stress; pub use stress::Stress;` + docs table row |
| regenerate | `examples/playground/generated/**` | `keel bindgen` (below); goldens of the playground only |
| new | `examples/playground/web/src/views/StressView.tsx`, `src/stress-stats.ts` | 8.2 |
| modify | `examples/playground/web/src/App.tsx`, `src/index.css` | the tab and styles |
| new | `examples/playground/web/src/stress-stats.test.ts` | vitest: `percentile`, `WindowStats` rates, `FrameMonitor` with a fake rAF clock (60 Hz and 120 Hz, drops counted right) |
| modify | `examples/playground/web/smoke/*` | Playwright: open `?screen=stress&mode=firehose&rate=10000&autostart=1`, wait 2 s, the Received tile is > 0 and no console error |
| modify (D2) | `runtimes/ts/@keel/runtime/src/mirror.ts`, `test/mirror.test.ts`, `docs/SPEC.md` 17.1 | `addDrainListener` + counters `entriesReceived`, `entriesApplied`, `drains`; tests: listener called once per flush with the right counts, removal, no timing when no listener |
| modify | `site/data/bench.json` | the harsh rows (7.8) in site-v2's shape |
| new | `.10x/decisions/sde/stress.md` | the implementer's decision record (what was re-baselined, what was dropped, numbers) |

Out of S1's ownership unless the integrator says otherwise: `crates/keel-ffi/tests/commit_alloc.rs` (D4).

### 9.3 What "done" means for S1

1. Every row of section 6 exists and passes on the reference machine, re-baselined where marked, with the
   `measured` values that the harness itself printed; `the_budget_file_covers_every_workload_and_nothing_else`
   and `stress_table_covers_every_scenario` pass.
2. `cargo test -p keel-bench --test stress --release` passes three times in a row locally; the soak passes
   for 60 s locally and 10 s in CI; each scenario's invariants fail when deliberately broken (try once by hand:
   make `ApplyingHost` skip one patch and see the equality invariant fail; make the drain thread swap two
   change-sets and see out-of-order fail) and that is described in the decision record.
3. `cargo test --workspace` (debug) stays green and does not get slower by more than ~5 s (debug runs smoke
   only).
4. The playground core's tests pass, bindings are regenerated and `--check` clean, and the contract
   scenarios still pass on all three platforms (51/51): adding a store changes the playground's schema hash.
5. The web stress screen runs in Chrome at 100 k/s firehose for 60 s without an error, the numbers in the
   tiles agree with the embed `postMessage` payload, and a screenshot plus the numbers (browser, machine,
   date) are in RESULTS.md. Smoke test passes headless.
6. RESULTS.md "Harsh conditions" and `site/data/bench.json` rows carry the same numbers.
7. Lints: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo doc --no-deps`
   warning-free (`#![deny(missing_docs)]` holds in `bench/src`), `npm run typecheck` in the web app and the TS
   runtime.

### 9.4 Commands

```bash
source /Users/shrey/Desktop/src/keel/scripts/env.sh

# Rust: lint, unit + smoke (debug), gates (release)
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p keel-bench
cargo test -p keel-bench --test budgets --release -- --nocapture
cargo test -p keel-bench --test stress --release -- --nocapture
KEEL_STRESS_SECONDS=10 cargo test -p keel-bench --test stress --release -- --nocapture   # RESULTS.md numbers

# Baselines (print rows in budgets.toml syntax)
KEEL_BENCH_FILTER=stress/ cargo test -p keel-bench --test budgets --release -- --ignored --nocapture baseline
cargo test -p keel-bench --test stress --release -- --ignored --nocapture stress_baseline

# Soak
cargo run -p keel-bench --release --bin soak -- --seconds 60 --json target/soak.json
cargo run -p keel-bench --release --bin soak -- --seconds 10

# Criterion, for humans
cargo bench -p keel-bench --bench stress

# Playground core, bindings, apps, contracts
cargo test -p playground-core
cargo run -p keel-cli -- bindgen -C examples/playground --docs
cargo run -p keel-cli -- bindgen -C examples/playground --docs --check
cargo run -p keel-cli -- build -C examples/playground --platform web
(cd examples/playground/web && npm ci && npm run typecheck && npm test && npm run smoke)
(cd runtimes/ts/@keel/runtime && npm ci && npm run typecheck && npm test)
bash contract-tests/run-all.sh

# Whole workspace before the review
cargo test --workspace
```

(After the rename piece lands, `runtimes/ts/@keel/runtime` is at its new path; use it.)

### 9.5 Traps for the implementer

* Build fixtures inside the binary that uses them (the `#[path]` inclusion), never in `bench/src`: the
  macro-generated registrations must not be dropped by the linker (`bench/common/mod.rs:1-5`).
* `Runtime::new` + drop without `shutdown()` leaks the runtime and its threads (RESULTS.md finding 3): use
  `Core` (`host.rs:80-102`) or call `shutdown()`.
* Writes outside a core-lock holder trip the debug write checker (ADR-023): cell-level scenarios call
  `drive_from_this_thread()` first; runtime scenarios write inside dispatched calls or tasks only.
* Host callbacks must not call back into the runtime (E_REENTRANT, SPEC 5.1): `DrainHost` only copies and
  queues; completer threads call `port_reply` from their own threads.
* Keep timing out of the setup: seeding 10,000 rows or attaching 100,000 signals happens before the clock
  starts; RSS sampling (a process spawn on macOS) happens outside timed regions.
* Churn must keep the list length stable (the fixed cycle) or a 10 s run drifts into a different list size.
* The stress screen's generator must be stopped when the tab unmounts (`useEffect` cleanup calls `stop()`
  then `close()`), or the next tab inherits a running firehose.

## 10. Decisions for the integrator

* **D1. Accept ADR-031's direction?** Platform-side, frame-aligned merge with a bounded backlog (recommended)
  versus core-side coalescing (rejected in the ADR, kept as the fallback if device numbers show the FFI copy
  dominating). S2 cannot start before this.
* **D2. Let S1 add `Mirror.addDrainListener` and three counters to the TS runtime** (additive public API,
  no behaviour change, outside S1's `bench/**` + playground ownership). Recommended: yes, it is ADR-031
  decision 5 pulled forward, and it is what makes the live numbers honest. Fallback: the view wraps
  `mirror.flush`.
* **D3. Where do the landing page's live numbers come from before S2?** Recommended: ship S1's screen
  against today's runtime (wasm-main batches per timer tick already, so firehose at 100 k/s is presentable;
  keyed churn at 1 M/s drops frames today) and swap the numbers when S2 lands, saying "before/after ADR-031"
  in RESULTS.md. Alternative: hold the stress screen's landing-page link until S2.
* **D4. Allocation gate in `keel-ffi`** (`crates/keel-ffi/tests/commit_alloc.rs`: at most 3 allocations per
  observed single-signal commit, 0 unobserved) and a follow-up to make it 0 (section 11). Outside `bench/**`.
* **D5. CI time**: layer B at 2 s x 5 scenarios + a 10 s soak adds about a minute to `bench.yml`. Accept, or
  run the soak on `main` only.
* **D6. Split S1** into S1a (`bench/**`) and S1b (playground core + web + TS hook) if one implementer
  cannot carry both in one review; S1a has no dependency on site-v2, S1b does (URL parameters, the stats
  message, `bench.json`).

## 11. Findings outside this piece

1. **Three allocations per observed commit** (`txn.rs:258, 280`, `store.rs:675`), zero unobserved. At 100 k
   commits/s that is 300 k `malloc`/`free` pairs per second; ADR-028 showed the allocator was 60% of the sync
   call on this OS. Reusing per-thread vectors (as `take_buffer` already does for the payload,
   `txn.rs:360-374`) is internal to `keel-signals` (no ADR); ADR-028 took 40% off the sync call by removing
   allocations the same way, so a similar share of the 82 ns is likely. A follow-up task, gated by D4's test.
2. **Kotlin's patch apply copies the list per change-set** and TS's does too; ADR-031 fixes both through the
   merge, but even one patch per frame on a 100,000-row list is a 100,000-element copy per frame on those two
   platforms. An in-place apply for lists the mirror created in the same drain is possible later (TS: safe
   while notifications are deferred to the end of the batch), not needed for the budgets here.
3. **Cross-store tearing**: a transaction that touches several stores reaches the platform as several
   change-sets, and a drain can fall between them (today and after ADR-031). Worth a line in SPEC 11; a fix
   would need the mirror to know a transaction's store count (a wire change), not proposed.
