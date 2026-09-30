# Architect: the harsh-conditions benchmark (branch `wt/stress`, design only)

Full design and implementation brief: `.10x/specs/2026-09-30-stress-bench-design.md`. Draft ADR:
`.10x/adrs/ADR-031-frame-coalesced-delivery.md` (proposed).

## Decisions

* **Measure the core and the platform separately, and say which is which.** The core is fast enough by two
  orders of magnitude (82 ns per observed transaction, 12 M/s on one host core); the risk under high-frequency
  data is on the platform side. Host numbers are labelled "core side"; the platform apply is measured live in
  the browser (and on devices in the device phase), never on a CI runner or a simulator.
* **Two layers in `bench/`.** Per-operation p50 rows join the existing gate unchanged (`[bench."stress/..."]`,
  5x rule); sustained scenarios get a new `[stress."..."]` table kind with throughput floors, p99/p999
  ceilings, exact bytes, RSS growth, and invariants asserted in code (nothing lost, nothing reordered, the host
  list equals the core list, a stream never more than one item ahead). A soak binary covers leaks and drift.
  No new dependency: RSS from `/proc/self/status` or `ps`, a fixed log-linear histogram in `bench/src`.
* **Scenarios**: firehose (core- and host-driven, plus an event-port variant), keyed churn on 10,000 rows with
  a fixed length-preserving op cycle, fan-out (100 k observed / 1% dirty, the 10 k pair that shows O(dirty),
  and 1,000 stores), stream backpressure reshaped to "never more than one item beyond credit" (Keel's streams
  pull), concurrent completions (8 threads, 256 in flight, a 60 Hz drain), soak (mixed paced load).
* **The playground's stress screen generates in the core**, paced by the `Timer` port and compensated by the
  `Clock` port, seeded, no ambient randomness (R12): a host loop would measure the read-your-writes path, not
  the firehose. It reports through the site-v2 `keel-stats` message with optional extra fields.
* **Frame-coalesced delivery needs an ADR.** Nothing coalesces across transactions between the core and the
  UI; the platforms coalesce the hop, not the work, with unbounded queues, an O(list) copy per keyed patch
  (TS, Kotlin) and one `postMessage` per change-set in TS worker mode. ADR-031 proposes a platform-side,
  frame-aligned, byte-level merge per signal with a bounded backlog and read-your-writes on replies; it
  rejects core-side coalescing (reopens ADR-019/020/023/027, adds ABI, loses per-transaction change-sets)
  and producer backpressure (state is last-writer-wins; blocking the committer stalls the core).

## Why

The founder's question is "does it survive very high-frequency data". The honest answer from the code is
"the core does, the platform delivery path does not yet bound its work or memory", and a benchmark that only
timed the core would have hidden that. The design makes the gap measurable (the stress screen, before and
after), fixes it with the smallest change that keeps every wire and ordering guarantee (ADR-031), and gates
the core side in CI so it stays fast.

## Open for the integrator

D1 accept ADR-031's direction; D2 let S1 add `Mirror.addDrainListener` to the TS runtime; D3 landing-page
numbers before or after S2; D4 an allocation gate in `keel-ffi` (3 allocations per observed commit today);
D5 about a minute more CI; D6 split S1 into bench and playground halves. Details in section 10 of the spec.
