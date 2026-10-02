//! Harsh conditions: what the core does at rates far above any UI, for seconds at a time.
//!
//! Two layers share the fixtures in `fixtures.rs` and the hosts in `host.rs`:
//!
//! * **Layer A**, [`workloads`]: the unit of work of each scenario as a per-operation
//!   [`Workload`], gated by the `[bench."stress/..."]` rows of `budgets.toml` with the same p50
//!   rule as every other row (5x the measured p50). It catches "an operation became several
//!   times slower".
//! * **Layer B**, [`scenarios`]: each scenario runs for a fixed wall time with its producers,
//!   consumers and threads, times every operation into a
//!   [`Histogram`], counts change-set bytes, samples RSS and
//!   checks invariants (nothing lost, nothing reordered, the host's list equals the core's, a
//!   stream never more than one item ahead of its credit). It is gated by the
//!   `[stress."..."]` tables. It catches "fast once, but not for two seconds": tails, leaks,
//!   reordering and loss.
//!
//! The numbers are the **core side** of the pipeline, on a host. What the platform does with a
//! change-set after the FFI callback (decode, apply, render) is measured on the platform; see
//! `bench/RESULTS.md`, "Harsh conditions".
//!
//! Design: `.10x/specs/2026-09-30-stress-bench-design.md`.
#![allow(missing_docs, dead_code)]

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use undra::runtime::testing::{drive_from_this_thread, port_reply_ok};
use undra::signals::{ALL_SIGNALS, ChangeSink, Signal, StoreCell, txn, with_sink};
use undra::wire::{Encode, Handle, Writer};
use undra_bench::rss::{RssGrowth, RssSeries};
use undra_bench::stats::Histogram;
use undra_bench::workload::{Workload, plain};

use super::fixtures::{
    Churn, ChurnViews, Fetcher, Producer, SOURCE_PORT, TickSink, Ticker, open_by_title, tick_event,
};
use super::host::{
    ApplyingHost, CopyingHost, Core, CountingHost, DrainHost, ListMirror, MainStats, MainThread,
    call_ok, construct, method_call, runtime, runtime_with,
};

fn enc<T: Encode>(value: &T) -> Vec<u8> {
    value.encode_to_vec()
}

/// Rows of the keyed list the churn scenario changes: 10,000, the blueprint's list. An
/// unoptimised build smoke-runs on 500 (its list operations are hundreds of times slower, and
/// only the invariants are checked there).
pub const CHURN_ROWS: u32 = if cfg!(debug_assertions) { 500 } else { 10_000 };
/// Calls kept in flight by the completions scenario.
pub const WINDOW: u64 = 256;
/// Host threads answering port calls in the completions scenario.
pub const COMPLETERS: usize = 8;
/// Calls the completions issuer makes between two looks at the clock.
const CLOCK_EVERY: usize = 32;
/// Bytes of a change-set holding one `u64` entry: 12 header, 17 entry header, 8 value.
pub const TICK_BYTES: u64 = 37;
/// Bytes of change-sets per keyed-churn operation: the fixed cycle (4 updates, 2 inserts, 2 removes,
/// 2 moves) at fixed widths ships 610 bytes per round of ten, whatever the positions.
pub const CHURN_BYTES_PER_OP: u64 = 61;
/// Bytes of a change-set holding `n` entries of a `u32` each: 12 header plus 21 per entry.
pub const fn u32_change_set_bytes(entries: u64) -> u64 {
    12 + entries * 21
}

// ---------------------------------------------------------------------------------------------
// Layer A: per-operation workloads
// ---------------------------------------------------------------------------------------------

/// The nine layer-A rows (`stress/...`), added to `workloads::all()` and `group("stress")`.
pub fn workloads() -> Vec<Workload> {
    vec![
        Workload::new("stress/firehose/txn_x1000", firehose_burst),
        Workload::new("stress/firehose/call_set", firehose_call),
        Workload::new("stress/firehose/event", firehose_event),
        Workload::new("stress/keyed_churn_10k/ops_x1000", churn_ops),
        Workload::new("stress/derived_churn_10k/ops_x1000", derived_churn_ops),
        Workload::new("stress/fanout/100k_observed_1k_dirty", || {
            fanout_cell(100_000, 1_000)
        }),
        Workload::new("stress/fanout/10k_observed_1k_dirty", || {
            fanout_cell(10_000, 1_000)
        }),
        Workload::new("stress/fanout/1k_stores_1k_dirty", fanout_stores_workload),
        Workload::new("stress/stream/items_x1000", stream_items),
    ]
}

/// 1,000 implicit transactions on one observed `Signal<u64>` in one call: a core-side producer.
fn firehose_burst() -> Box<dyn undra_bench::workload::Bench> {
    let (rt, host) = runtime();
    let ticker = construct(&rt, "Ticker", &[]);
    rt.observe(ticker.0, ALL_SIGNALS, true);
    let payload = method_call(ticker, "Ticker", "burst", 2, &enc(&1_000_u32));
    let (sets, bytes) = (host.change_sets(), host.change_set_bytes());
    call_ok(&rt, &payload);
    assert_eq!(
        host.change_sets() - sets,
        1_000,
        "one change-set per transaction"
    );
    assert_eq!(
        host.change_set_bytes() - bytes,
        1_000 * TICK_BYTES,
        "each change-set is one 37-byte entry"
    );
    plain(move || {
        black_box(rt.call_sync(black_box(&payload)));
    })
}

/// One `Ticker.set(v)` through `Runtime::call_sync`: a host pushing updates one call at a time.
fn firehose_call() -> Box<dyn undra_bench::workload::Bench> {
    let (rt, host) = runtime();
    let ticker = construct(&rt, "Ticker", &[]);
    rt.observe(ticker.0, ALL_SIGNALS, true);
    let payloads: Vec<Vec<u8>> = (0..1024_u64)
        .map(|i| method_call(ticker, "Ticker", "set", 2, &enc(&(i + 1))))
        .collect();
    let (sets, bytes) = (host.change_sets(), host.change_set_bytes());
    call_ok(&rt, &payloads[0]);
    assert_eq!(host.change_sets() - sets, 1, "one change-set per call");
    assert_eq!(host.change_set_bytes() - bytes, TICK_BYTES);
    let mut next = 0_usize;
    plain(move || {
        next = (next + 1) & 1023;
        black_box(rt.call_sync(black_box(&payloads[next])));
    })
}

/// One event through `Runtime::event` whose subscriber writes an observed signal: the path a
/// WebSocket or sensor feed takes into the core.
fn firehose_event() -> Box<dyn undra_bench::workload::Bench> {
    let (rt, host) = runtime();
    let sink = construct(&rt, "TickSink", &[]);
    rt.observe(sink.0, ALL_SIGNALS, true);
    let events: Vec<(u32, u32, Vec<u8>)> = (0..1024_u64).map(|i| tick_event(i + 1)).collect();
    let (sets, bytes) = (host.change_sets(), host.change_set_bytes());
    let (port, method, payload) = &events[0];
    rt.event(*port, *method, payload);
    assert_eq!(host.change_sets() - sets, 1, "one change-set per event");
    assert_eq!(host.change_set_bytes() - bytes, TICK_BYTES);
    assert_eq!(
        rt.object::<TickSink>(sink.0).expect("the sink").current(),
        1
    );
    let mut next = 0_usize;
    plain(move || {
        next = (next + 1) & 1023;
        let (port, method, payload) = &events[next];
        rt.event(*port, *method, black_box(payload));
    })
}

/// 1,000 recorded list operations on a 10,000-row keyed list, each its own transaction and
/// applied to a host-side list.
fn churn_ops() -> Box<dyn undra_bench::workload::Bench> {
    let (rt, host, churn) = churn_runtime();
    let payload = method_call(churn, "Churn", "churn", 3, &enc(&1_000_u32));
    let sets = host.counts.change_sets();
    call_ok(&rt, &payload);
    assert_eq!(
        host.counts.change_sets() - sets,
        1_000,
        "one change-set per operation"
    );
    assert_churn_in_sync(&rt, &host, churn);
    plain(move || {
        black_box(rt.call_sync(black_box(&payload)));
    })
}

/// A runtime whose host applies every change-set to a mirror of a seeded, observed `Churn`.
fn churn_runtime() -> (Arc<Core>, Arc<ApplyingHost>, Handle) {
    let host = Arc::new(ApplyingHost::default());
    let rt = runtime_with(host.clone(), 0);
    let churn = construct(&rt, "Churn", &[]);
    call_ok(
        &rt,
        &method_call(churn, "Churn", "seed", 2, &enc(&CHURN_ROWS)),
    );
    // The rows are the store's first (and only) signal.
    host.watch(ListMirror::new(churn, 0));
    rt.observe(churn.0, ALL_SIGNALS, true);
    host.with_mirror(|m| assert_eq!(m.list.len(), CHURN_ROWS as usize, "the initial emission"));
    (rt, host, churn)
}

/// The host's list must be the core's list, row for row and field for field.
fn assert_churn_in_sync(rt: &Core, host: &ApplyingHost, churn: Handle) {
    let core = rt.object::<Churn>(churn.0).expect("the store").rows_now();
    host.with_mirror(|m| {
        assert_eq!(m.errors, 0, "a patch failed to apply");
        assert!(m.list == core, "the host list must equal the core list");
    });
    assert_eq!(
        core.len(),
        CHURN_ROWS as usize,
        "the cycle keeps the length"
    );
}

/// 1,000 recorded operations on `ChurnViews`, each its own transaction: the source and its sorted
/// derived view both observed and applied to host-side lists.
fn derived_churn_ops() -> Box<dyn undra_bench::workload::Bench> {
    let (rt, host, views) = derived_churn_runtime();
    let payload = method_call(views, "ChurnViews", "churn", 3, &enc(&1_000_u32));
    let sets = host.counts.change_sets();
    call_ok(&rt, &payload);
    assert_eq!(
        host.counts.change_sets() - sets,
        1_000,
        "one change-set per operation"
    );
    let store = rt.object::<ChurnViews>(views.0).expect("the store");
    host.with_mirror_at(1, |m| {
        assert_eq!(m.errors, 0, "a view patch failed to apply");
        assert!(
            m.list == open_by_title(&store.rows_now()),
            "the host's view is filter + stable sort"
        );
        assert_eq!(m.fulls, 1, "the view was sent whole once, at the observe");
    });
    plain(move || {
        black_box(rt.call_sync(black_box(&payload)));
    })
}

/// A runtime whose host mirrors a seeded, observed `ChurnViews`: its rows (signal 0) and its
/// derived view `open` (signal 1, the rows not done by title).
fn derived_churn_runtime() -> (Arc<Core>, Arc<ApplyingHost>, Handle) {
    let host = Arc::new(ApplyingHost::default());
    let rt = runtime_with(host.clone(), 0);
    let views = construct(&rt, "ChurnViews", &[]);
    call_ok(
        &rt,
        &method_call(views, "ChurnViews", "seed", 2, &enc(&CHURN_ROWS)),
    );
    host.watch(ListMirror::new(views, 0));
    host.watch(ListMirror::new(views, 1));
    rt.observe(views.0, ALL_SIGNALS, true);
    host.with_mirror_at(1, |m| {
        assert_eq!(
            m.list.len(),
            CHURN_ROWS as usize,
            "the initial emission of the view"
        );
    });
    (rt, host, views)
}

/// A sink that counts what it is given.
#[derive(Default)]
struct CountSink {
    change_sets: AtomicU64,
    bytes: AtomicU64,
}

impl ChangeSink for CountSink {
    fn deliver(&self, change_set: &[u8]) {
        self.change_sets.fetch_add(1, Ordering::Relaxed);
        self.bytes
            .fetch_add(change_set.len() as u64, Ordering::Relaxed);
    }
}

/// One store cell with `observed` observed signals; each transaction writes `dirty` of them,
/// evenly spread and rotating: the signal-table layer, no runtime, no dispatch.
struct CellFanout {
    _cell: Arc<StoreCell>,
    signals: Vec<Signal<u32>>,
    sink: Arc<CountSink>,
    stride: usize,
    dirty: usize,
    offset: usize,
}

impl CellFanout {
    fn new(observed: usize, dirty: usize) -> CellFanout {
        // Writes outside a core-lock holder trip the debug write checker (ADR-023).
        drive_from_this_thread();
        let cell = StoreCell::new(0xBE_C0_10);
        let signals: Vec<Signal<u32>> = (0..observed).map(|_| Signal::new(0)).collect();
        for (id, signal) in signals.iter().enumerate() {
            cell.attach(signal, id as u32).expect("attach");
        }
        cell.set_handle(Handle::new(1, 1).0);
        cell.observe(ALL_SIGNALS, true, &mut Writer::new());
        CellFanout {
            _cell: cell,
            signals,
            sink: Arc::new(CountSink::default()),
            stride: observed / dirty,
            dirty,
            offset: 0,
        }
    }

    /// One transaction writing `dirty` signals.
    fn transaction(&mut self) {
        let signals = &self.signals;
        let (stride, dirty, offset) = (self.stride, self.dirty, self.offset);
        with_sink(self.sink.clone(), || {
            txn(|| {
                for i in 0..dirty {
                    signals[offset + i * stride].update(|n| *n = n.wrapping_add(1));
                }
            });
        });
        self.offset = (offset + 1) % stride;
    }

    fn change_sets(&self) -> u64 {
        self.sink.change_sets.load(Ordering::Relaxed)
    }

    fn bytes(&self) -> u64 {
        self.sink.bytes.load(Ordering::Relaxed)
    }
}

/// One transaction writing 1,000 of `observed` observed signals of one store cell: one
/// change-set of 21,012 bytes whatever `observed` is.
fn fanout_cell(observed: usize, dirty: usize) -> Box<dyn undra_bench::workload::Bench> {
    let mut fan = CellFanout::new(observed, dirty);
    fan.transaction();
    assert_eq!(fan.change_sets(), 1, "one transaction, one change-set");
    assert_eq!(
        fan.bytes(),
        u32_change_set_bytes(dirty as u64),
        "the change-set carries the dirty signals and nothing else"
    );
    plain(move || fan.transaction())
}

/// `stores` store cells of `per_store` observed signals each, written one signal per store in one
/// transaction: one change-set per store, all sharing the transaction.
struct StoresFanout {
    _cells: Vec<Arc<StoreCell>>,
    signals: Vec<Signal<u32>>,
    sink: Arc<CountSink>,
    stores: usize,
    per_store: usize,
    offset: usize,
}

impl StoresFanout {
    fn new(stores: usize, per_store: usize) -> StoresFanout {
        drive_from_this_thread();
        let mut cells = Vec::with_capacity(stores);
        let mut signals = Vec::with_capacity(stores * per_store);
        for store in 0..stores {
            let cell = StoreCell::new(0xBE_C0_11);
            for id in 0..per_store {
                let signal = Signal::new(0_u32);
                cell.attach(&signal, id as u32).expect("attach");
                signals.push(signal);
            }
            cell.set_handle(Handle::new(store as u32 + 1, 1).0);
            cell.observe(ALL_SIGNALS, true, &mut Writer::new());
            cells.push(cell);
        }
        StoresFanout {
            _cells: cells,
            signals,
            sink: Arc::new(CountSink::default()),
            stores,
            per_store,
            offset: 0,
        }
    }

    fn transaction(&mut self) {
        let signals = &self.signals;
        let (stores, per_store, offset) = (self.stores, self.per_store, self.offset);
        with_sink(self.sink.clone(), || {
            txn(|| {
                for store in 0..stores {
                    signals[store * per_store + offset].update(|n| *n = n.wrapping_add(1));
                }
            });
        });
        self.offset = (offset + 1) % per_store;
    }

    fn change_sets(&self) -> u64 {
        self.sink.change_sets.load(Ordering::Relaxed)
    }

    fn bytes(&self) -> u64 {
        self.sink.bytes.load(Ordering::Relaxed)
    }
}

fn fanout_stores_workload() -> Box<dyn undra_bench::workload::Bench> {
    let mut fan = StoresFanout::new(1_000, 100);
    fan.transaction();
    assert_eq!(fan.change_sets(), 1_000, "one change-set per store");
    assert_eq!(fan.bytes(), 1_000 * u32_change_set_bytes(1));
    plain(move || fan.transaction())
}

/// A runtime (no core thread) with an always-ready stream open on a `Producer`, and the call id
/// of that stream.
fn open_stream(host: &Arc<CountingHost>) -> (Arc<Core>, Arc<Producer>, u32) {
    let rt = runtime_with(host.clone(), 0);
    let producer = construct(&rt, "Producer", &[]);
    let object = rt.object::<Producer>(producer.0).expect("the producer");
    let call_id = 7;
    let payload = method_call(producer, "Producer", "numbers", call_id, &enc(&u64::MAX));
    assert_eq!(rt.call(&payload), 0, "the stream call is accepted");
    rt.run_pending();
    (rt, object, call_id)
}

/// 1,000 stream items: grant 1,000 credits, run the executor until idle.
fn stream_items() -> Box<dyn undra_bench::workload::Bench> {
    let host = Arc::new(CountingHost::default());
    let (rt, _producer, call_id) = open_stream(&host);
    let delivered = host.stream_items();
    rt.stream_credit(call_id, 1_000);
    rt.run_pending();
    assert_eq!(
        host.stream_items() - delivered,
        1_000,
        "a grant of 1,000 delivers exactly 1,000 items"
    );
    plain(move || {
        rt.stream_credit(call_id, 1_000);
        rt.run_pending();
    })
}

// ---------------------------------------------------------------------------------------------
// Layer B: sustained scenarios
// ---------------------------------------------------------------------------------------------

/// Something deliberately broken so a test can show an invariant has teeth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fault {
    /// Nothing is broken.
    #[default]
    None,
    /// The keyed-churn host drops every 101st patch: the host list must stop equalling the
    /// core list.
    SkipPatches,
    /// The keyed-churn host drops one patch only, the first: an `Update` (the cycle starts with
    /// one), which leaves the row ids intact and which a later update of the same row repairs.
    /// The scenario must still fail.
    DropOneUpdate,
    /// The "main thread" swaps the first two change-sets of its first frame: delivery order
    /// must be reported as broken (and, for a store whose writes each add one, the sequence).
    SwapChangeSets,
    /// The "main thread" never looks at the second change-set of its first frame: a change-set
    /// lost on the way to the UI must be reported (the walked count and the sequence).
    DropChangeSet,
}

/// How a sustained scenario runs.
#[derive(Clone, Copy, Debug)]
pub struct StressConfig {
    /// Wall time of the measured run (2 s in CI, 10 s for the numbers in RESULTS.md; 100 ms in
    /// the debug smoke run).
    pub duration: Duration,
    /// Sample resident memory (a process spawn on macOS: only release runs do).
    pub rss: bool,
    /// Something to break on purpose; [`Fault::None`] outside the invariant tests.
    pub fault: Fault,
    /// How long each scenario runs **before** the measured run, results discarded: thread start-up,
    /// cold caches, the first allocations. `None` is [`default_warmup`] (a tenth of the run, at
    /// most 200 ms); `Some(Duration::ZERO)` measures from the first operation. The measured
    /// `duration` is unchanged, so a run lasts `warmup + duration`.
    pub warmup: Option<Duration>,
    /// Operations the measured run does **at least**, whatever its duration: a run whose time is up
    /// before it has done that many goes on until it has, so a machine that is slow (or busy) takes
    /// longer instead of reporting less. The tests that need a fault to fire (every 101st patch is
    /// dropped) or a scenario to have done something ask for the count they need. `0` is a run of
    /// `duration` alone. A run that has not reached it [`GIVE_UP`] after its time is up stops with
    /// what it has: a hang detector, never a performance claim (the report's `ops` is then below
    /// the floor, and the test that asked says so).
    pub min_ops: u64,
}

/// How long past its duration a run waits for its [`min_ops`](StressConfig::min_ops) before it gives
/// up. It only detects a run that cannot make progress at all, so it is generous.
pub const GIVE_UP: Duration = Duration::from_secs(120);

/// Whether a run is over: its time is up and it has done at least `min_ops` operations (or has
/// waited [`GIVE_UP`] for them).
fn run_is_over(now: Instant, deadline: Instant, ops: u64, min_ops: u64) -> bool {
    now >= deadline && (ops >= min_ops || now >= deadline + GIVE_UP)
}

/// The warm-up of a run of `duration` that nobody configured: a tenth of it, at most 200 ms (a 10 s
/// run warms up for 200 ms, a 2 s CI run for 200 ms, the 100 ms debug smoke run for 10 ms).
///
/// # Example
///
/// ```ignore
/// assert_eq!(default_warmup(Duration::from_secs(10)), Duration::from_millis(200));
/// assert_eq!(default_warmup(Duration::from_millis(100)), Duration::from_millis(10));
/// ```
pub fn default_warmup(duration: Duration) -> Duration {
    (duration / 10).min(Duration::from_millis(200))
}

impl StressConfig {
    /// A run of `duration` with RSS sampling on, the default warm-up and nothing broken.
    pub fn new(duration: Duration) -> StressConfig {
        StressConfig {
            duration,
            rss: true,
            fault: Fault::None,
            warmup: None,
            min_ops: 0,
        }
    }

    /// The warm-up before a measured phase of `span`: what was configured, else
    /// [`default_warmup`] of the phase.
    pub fn warmup_for(&self, span: Duration) -> Duration {
        self.warmup.unwrap_or_else(|| default_warmup(span))
    }

    /// The warm-up of the whole run (see [`warmup_for`](StressConfig::warmup_for)).
    pub fn warmup(&self) -> Duration {
        self.warmup_for(self.duration)
    }
}

/// A property the scenario checks about its own run.
#[derive(Clone, Debug)]
pub struct Invariant {
    /// The property, in words, with the numbers that were compared.
    pub what: String,
    /// Whether it held.
    pub holds: bool,
    /// A comparison of timings rather than a correctness property: a noisy machine can break
    /// it, so it is gated like a budget (retried, and skipped by the debug smoke run), never
    /// treated as an invariant.
    pub timing: bool,
}

impl Invariant {
    fn new(what: impl Into<String>, holds: bool) -> Invariant {
        Invariant {
            what: what.into(),
            holds,
            timing: false,
        }
    }

    fn timing(what: impl Into<String>, holds: bool) -> Invariant {
        Invariant {
            timing: true,
            ..Invariant::new(what, holds)
        }
    }
}

/// What one sustained run found.
#[derive(Clone, Debug)]
pub struct StressReport {
    /// The `[stress."name"]` key.
    pub name: &'static str,
    /// Operations completed in the measured run.
    pub ops: u64,
    /// Wall time of the measured run (the warm-up is not in it).
    pub elapsed: Duration,
    /// How long the scenario ran before the measured run, results discarded (zero for the stream,
    /// whose own slow half and RSS warm-up do that job).
    pub warmup: Duration,
    /// Per-operation latency; empty where the scenario has none (the stream).
    pub latency: Histogram,
    /// Change-set (or stream item) bytes over the run.
    pub bytes: u64,
    /// RSS growth after warm-up, when it was sampled.
    pub rss: Option<RssGrowth>,
    /// What the scenario checked about itself.
    pub invariants: Vec<Invariant>,
    /// Facts worth printing: queue depths, the 10k companion run, the stream's max-ahead.
    pub notes: Vec<String>,
}

impl StressReport {
    /// Operations per second of wall time.
    pub fn per_sec(&self) -> f64 {
        self.ops as f64 / self.elapsed.as_secs_f64()
    }

    /// Bytes per operation.
    pub fn bytes_per_op(&self) -> f64 {
        if self.ops == 0 {
            0.0
        } else {
            self.bytes as f64 / self.ops as f64
        }
    }

    /// The invariants that did not hold (timing comparisons excluded: see
    /// [`timing_failures`](StressReport::timing_failures)).
    pub fn broken(&self) -> Vec<&Invariant> {
        self.invariants
            .iter()
            .filter(|i| !i.holds && !i.timing)
            .collect()
    }

    /// The timing comparisons that did not hold: gate failures, retried on a noisy run.
    pub fn timing_failures(&self) -> Vec<&Invariant> {
        self.invariants
            .iter()
            .filter(|i| !i.holds && i.timing)
            .collect()
    }

    /// A latency percentile in nanoseconds, if the scenario times operations.
    pub fn percentile(&self, p: f64) -> Option<u64> {
        (self.latency.count() > 0).then(|| self.latency.percentile(p))
    }
}

/// A sustained scenario: runs to `cfg.duration` and reports.
pub type Scenario = fn(&StressConfig) -> StressReport;

/// The sustained scenarios in run order, by their `[stress."name"]` key.
pub fn scenarios() -> Vec<(&'static str, Scenario)> {
    vec![
        ("firehose/sustained", firehose),
        ("event/sustained", event_firehose),
        ("keyed_churn_10k/sustained", keyed_churn),
        ("derived_churn_10k/sustained", derived_churn),
        ("fanout/sustained", fanout),
        ("fanout_stores/sustained", fanout_stores),
        ("stream/backpressure", stream_backpressure),
        ("completions/8_threads", completions),
        ("completions/contended", completions_contended),
    ]
}

/// Scenarios whose change-set bytes per operation are the same on every run (their gate is the
/// exact measured value): the load is a fixed cycle at fixed widths, so a run of any whole number
/// of rounds ships the same bytes per operation.
pub const BYTES_EXACT: &[&str] = &[
    "firehose/sustained",
    "event/sustained",
    "keyed_churn_10k/sustained",
    "fanout/sustained",
    "fanout_stores/sustained",
    "completions/8_threads",
    "completions/contended",
];

pub fn nanos(d: Duration) -> u64 {
    d.as_nanos().min(u128::from(u64::MAX)) as u64
}

/// Runs `op` in rounds of `round` operations for `warmup` (at least one round unless `warmup` is
/// zero), recording nothing. Whole rounds, so a scenario whose operations come in a cycle ends the
/// warm-up where it started.
fn warm_up(warmup: Duration, round: usize, mut op: impl FnMut(usize)) -> u64 {
    if warmup.is_zero() {
        return 0;
    }
    let deadline = Instant::now() + warmup;
    let mut ops = 0_u64;
    loop {
        for i in 0..round {
            op(i);
        }
        ops += round as u64;
        if Instant::now() >= deadline {
            return ops;
        }
    }
}

/// Times `op` one call at a time until `duration` has passed and `min_ops` have been done, in
/// rounds of `round` operations (the clock is checked once per round).
fn time_ops(
    duration: Duration,
    min_ops: u64,
    round: usize,
    mut op: impl FnMut(usize),
) -> (Histogram, u64, Duration) {
    let mut hist = Histogram::new();
    let mut ops = 0_u64;
    let start = Instant::now();
    let deadline = start + duration;
    loop {
        for i in 0..round {
            let t0 = Instant::now();
            op(i);
            hist.record(nanos(t0.elapsed()));
        }
        ops += round as u64;
        if run_is_over(Instant::now(), deadline, ops, min_ops) {
            break;
        }
    }
    (hist, ops, start.elapsed())
}

fn warnings(host: &CountingHost) -> Invariant {
    Invariant::new(
        match host.first_warning() {
            None => "no warnings were logged (0)".to_owned(),
            Some(first) => format!(
                "no warnings were logged ({}; the first: {first})",
                host.warnings()
            ),
        },
        host.warnings() == 0,
    )
}

/// **Firehose** (a): one observed `Signal<u64>`, one implicit transaction per update, pushed one
/// `call_sync` at a time. Each transaction is one 37-byte change-set and O(1) in the core.
pub fn firehose(cfg: &StressConfig) -> StressReport {
    let host = Arc::new(CopyingHost::default());
    let rt = runtime_with(host.clone(), 0);
    let ticker = construct(&rt, "Ticker", &[]);
    rt.observe(ticker.0, ALL_SIGNALS, true);
    let payloads: Vec<Vec<u8>> = (0..1024_u64)
        .map(|i| method_call(ticker, "Ticker", "set", 2, &enc(&(i + 1))))
        .collect();
    warm_up(cfg.warmup(), payloads.len(), |i| {
        black_box(rt.call_sync(&payloads[i]));
    });
    let (sets, bytes) = (host.counts.change_sets(), host.counts.change_set_bytes());

    let (latency, ops, elapsed) = time_ops(cfg.duration, cfg.min_ops, payloads.len(), |i| {
        let reply = rt.call_sync(&payloads[i]);
        black_box(&reply);
    });

    let delivered = host.counts.change_sets() - sets;
    let shipped = host.counts.change_set_bytes() - bytes;
    let last = rt.object::<Ticker>(ticker.0).expect("the ticker").current();
    StressReport {
        name: "firehose/sustained",
        ops,
        elapsed,
        warmup: cfg.warmup(),
        latency,
        bytes: shipped,
        rss: None,
        invariants: vec![
            Invariant::new(
                format!("one change-set per transaction ({delivered} for {ops})"),
                delivered == ops,
            ),
            Invariant::new(
                format!("37 bytes per change-set ({shipped} for {ops})"),
                shipped == ops * TICK_BYTES,
            ),
            Invariant::new(
                format!("the last write is the signal's value ({last})"),
                last == payloads.len() as u64,
            ),
            warnings(&host.counts),
        ],
        notes: Vec::new(),
    }
}

/// **Event firehose** (g): `Runtime::event` on an event port whose subscriber writes a signal,
/// the path a WebSocket or sensor feed takes into the core. `event` takes the core lock on the
/// caller's thread, which is natural backpressure for a network thread.
pub fn event_firehose(cfg: &StressConfig) -> StressReport {
    let host = Arc::new(CopyingHost::default());
    let rt = runtime_with(host.clone(), 0);
    let sink = construct(&rt, "TickSink", &[]);
    rt.observe(sink.0, ALL_SIGNALS, true);
    let events: Vec<(u32, u32, Vec<u8>)> = (0..1024_u64).map(|i| tick_event(i + 1)).collect();
    warm_up(cfg.warmup(), events.len(), |i| {
        let (port, method, payload) = &events[i];
        rt.event(*port, *method, payload);
    });
    let (sets, bytes) = (host.counts.change_sets(), host.counts.change_set_bytes());

    let (latency, ops, elapsed) = time_ops(cfg.duration, cfg.min_ops, events.len(), |i| {
        let (port, method, payload) = &events[i];
        rt.event(*port, *method, payload);
    });

    let delivered = host.counts.change_sets() - sets;
    let shipped = host.counts.change_set_bytes() - bytes;
    let last = rt.object::<TickSink>(sink.0).expect("the sink").current();
    StressReport {
        name: "event/sustained",
        ops,
        elapsed,
        warmup: cfg.warmup(),
        latency,
        bytes: shipped,
        rss: None,
        invariants: vec![
            Invariant::new(
                format!("one change-set per event ({delivered} for {ops})"),
                delivered == ops,
            ),
            Invariant::new(
                format!("37 bytes per change-set ({shipped} for {ops})"),
                shipped == ops * TICK_BYTES,
            ),
            Invariant::new(
                format!("the last event is the signal's value ({last})"),
                last == events.len() as u64,
            ),
            warnings(&host.counts),
        ],
        notes: Vec::new(),
    }
}

/// **Keyed churn** (b): a 10,000-row keyed list changed one recorded operation per transaction
/// (the fixed 10-op cycle at seeded random positions), with a host-side list applying every
/// patch. Each timed step is commit, delivery and host apply of one operation; at the end the
/// host's list must be the core's list.
pub fn keyed_churn(cfg: &StressConfig) -> StressReport {
    let host = Arc::new(ApplyingHost::default());
    let rt = runtime_with(host.clone(), 0);
    let churn = construct(&rt, "Churn", &[]);
    call_ok(
        &rt,
        &method_call(churn, "Churn", "seed", 2, &enc(&CHURN_ROWS)),
    );
    let mut mirror = ListMirror::new(churn, 0);
    match cfg.fault {
        Fault::SkipPatches => mirror.skip_every(101),
        Fault::DropOneUpdate => mirror.skip_nth(1),
        Fault::None | Fault::SwapChangeSets | Fault::DropChangeSet => {}
    }
    host.watch(mirror);
    rt.observe(churn.0, ALL_SIGNALS, true);
    let seeded = host.with_mirror(|m| m.list.len());
    let call = method_call(churn, "Churn", "churn", 3, &enc(&1_u32));

    // Rounds of ten operations, so the list is back at its seeded length when the clock stops.
    let warm_ops = warm_up(cfg.warmup(), 10, |_| {
        black_box(rt.call_sync(&call));
    });
    let (sets, bytes) = (host.counts.change_sets(), host.counts.change_set_bytes());
    let (latency, ops, elapsed) = time_ops(cfg.duration, cfg.min_ops, 10, |_| {
        let reply = rt.call_sync(&call);
        black_box(&reply);
    });

    let delivered = host.counts.change_sets() - sets;
    let shipped = host.counts.change_set_bytes() - bytes;
    let core = rt.object::<Churn>(churn.0).expect("the store").rows_now();
    let (equal, host_rows, errors, patches, fulls) =
        host.with_mirror(|m| (m.list == core, m.list.len(), m.errors, m.patches, m.fulls));
    StressReport {
        name: "keyed_churn_10k/sustained",
        ops,
        elapsed,
        warmup: cfg.warmup(),
        latency,
        bytes: shipped,
        rss: None,
        invariants: vec![
            // Rows, not ids: an `Update` never changes an id, so an id comparison cannot see one
            // that went missing.
            Invariant::new(
                format!(
                    "the host list equals the core list, field for field ({host_rows} and {} rows)",
                    core.len()
                ),
                equal,
            ),
            // A lost patch can be repaired by a later write of the same row, so the end state
            // alone is not enough: every operation must have been applied as exactly one patch
            // (and nothing resynchronised the list with a full value after the first).
            Invariant::new(
                format!(
                    "every operation was applied on the host as one keyed patch ({patches} patches \
                     for {} operations, warm-up included; {fulls} full values, 1 expected)",
                    warm_ops + ops
                ),
                patches == warm_ops + ops && fulls == 1,
            ),
            Invariant::new(
                format!("{CHURN_BYTES_PER_OP} bytes per operation, exactly ({shipped} for {ops})"),
                shipped == CHURN_BYTES_PER_OP * ops,
            ),
            Invariant::new(
                format!(
                    "the list is back at {seeded} rows after {} operations ({})",
                    warm_ops + ops,
                    core.len()
                ),
                core.len() == seeded && seeded == CHURN_ROWS as usize,
            ),
            Invariant::new(
                format!("every patch applied on the host ({errors} failed)"),
                errors == 0,
            ),
            Invariant::new(
                format!("one change-set per operation ({delivered} for {ops})"),
                delivered == ops,
            ),
            warnings(&host.counts),
        ],
        notes: Vec::new(),
    }
}

/// **Derived churn** (ADR-039): `keyed_churn`'s list and cycle with a derived view of it, `open` =
/// the rows not done, sorted by title, both observed and mirrored on the host. Every `Update` of
/// the cycle toggles `done`, so it moves a row into or out of the view. Each timed step is the
/// commit (the source's op and the view's), delivery and both host applies. At the end the host's
/// view must be `filter + stable sort` of the core's rows, field for field.
pub fn derived_churn(cfg: &StressConfig) -> StressReport {
    let host = Arc::new(ApplyingHost::default());
    let rt = runtime_with(host.clone(), 0);
    let views = construct(&rt, "ChurnViews", &[]);
    call_ok(
        &rt,
        &method_call(views, "ChurnViews", "seed", 2, &enc(&CHURN_ROWS)),
    );
    let mut view = ListMirror::new(views, 1);
    match cfg.fault {
        Fault::SkipPatches => view.skip_every(101),
        Fault::DropOneUpdate => view.skip_nth(1),
        Fault::None | Fault::SwapChangeSets | Fault::DropChangeSet => {}
    }
    host.watch(ListMirror::new(views, 0));
    host.watch(view);
    rt.observe(views.0, ALL_SIGNALS, true);
    let seeded = host.with_mirror(|m| m.list.len());
    let call = method_call(views, "ChurnViews", "churn", 3, &enc(&1_u32));

    let warm_ops = warm_up(cfg.warmup(), 10, |_| {
        black_box(rt.call_sync(&call));
    });
    let (sets, bytes) = (host.counts.change_sets(), host.counts.change_set_bytes());
    let (latency, ops, elapsed) = time_ops(cfg.duration, cfg.min_ops, 10, |_| {
        let reply = rt.call_sync(&call);
        black_box(&reply);
    });

    let delivered = host.counts.change_sets() - sets;
    let shipped = host.counts.change_set_bytes() - bytes;
    let store = rt.object::<ChurnViews>(views.0).expect("the store");
    let core_rows = store.rows_now();
    let expected = open_by_title(&core_rows);
    let core_view = store.open_now();
    let stats = store.open_stats();
    let rows_equal = host.with_mirror(|m| m.list == core_rows && m.errors == 0);
    let (view_equal, view_len, view_errors, view_patches, view_seen, view_fulls, view_ops) = host
        .with_mirror_at(1, |m| {
            (
                m.list == expected,
                m.list.len(),
                m.errors,
                m.patches,
                m.patches_seen,
                m.fulls,
                m.ops,
            )
        });
    let all_ops = warm_ops + ops;
    StressReport {
        name: "derived_churn_10k/sustained",
        ops,
        elapsed,
        warmup: cfg.warmup(),
        latency,
        bytes: shipped,
        rss: None,
        invariants: vec![
            Invariant::new(
                format!(
                    "the host's view equals filter + stable sort of the core's rows, field for field \
                     ({view_len} and {} rows)",
                    expected.len()
                ),
                view_equal,
            ),
            Invariant::new(
                "the core's view (DerivedList::get) equals filter + stable sort of its rows",
                core_view == expected,
            ),
            Invariant::new(
                "the host's source list equals the core's, and every patch of it applied",
                rows_equal,
            ),
            Invariant::new(
                format!(
                    "every source operation produced at most two view ops ({view_ops} view ops for \
                     {all_ops} operations, warm-up included)"
                ),
                view_ops <= 2 * all_ops,
            ),
            Invariant::new(
                format!(
                    "every view entry after the first full value was a patch, applied ({view_patches} \
                     applied of {view_seen} delivered; {view_fulls} full values, 1 expected; the core \
                     rebuilt {} times and sent {} full values, 1 and 1 expected)",
                    stats.rebuilds, stats.full_values
                ),
                view_patches == view_seen
                    && view_fulls == 1
                    && stats.rebuilds == 1
                    && stats.full_values == 1,
            ),
            Invariant::new(
                format!("every view patch applied on the host ({view_errors} failed)"),
                view_errors == 0,
            ),
            Invariant::new(
                format!("one change-set per operation ({delivered} for {ops})"),
                delivered == ops,
            ),
            Invariant::new(
                format!(
                    "the list is back at {seeded} rows after {all_ops} operations ({})",
                    core_rows.len()
                ),
                core_rows.len() == seeded && seeded == CHURN_ROWS as usize,
            ),
            warnings(&host.counts),
        ],
        notes: vec![format!(
            "{:.1} bytes per operation (the source's op and, when the row is in the view, the view's)",
            shipped as f64 / ops.max(1) as f64
        )],
    }
}

/// Runs `CellFanout` transactions until `duration` has passed and `min_ops` have been done.
fn run_cell_fanout(
    fan: &mut CellFanout,
    duration: Duration,
    min_ops: u64,
) -> (Histogram, u64, Duration) {
    let mut hist = Histogram::new();
    let mut ops = 0_u64;
    let start = Instant::now();
    let deadline = start + duration;
    loop {
        let t0 = Instant::now();
        fan.transaction();
        let t1 = Instant::now();
        hist.record(nanos(t1 - t0));
        ops += 1;
        if run_is_over(t1, deadline, ops, min_ops) {
            break;
        }
    }
    (hist, ops, start.elapsed())
}

/// **Fan-out** (c): 100,000 observed signals in one store cell with 1,000 (1%) written per
/// transaction, then the same 1,000 writes over 10,000 observed. Commit time and bytes follow
/// the dirty count, not the observed count: 21,012 bytes in one change-set either way, and the
/// 100,000-signal commit costs under four times the 10,000-signal one. Measured at the
/// signal-table layer (no runtime), as `signals/changeset_100/cell` is: a `#[undra::store]` has
/// one field per signal.
pub fn fanout(cfg: &StressConfig) -> StressReport {
    let big_time = cfg.duration.mul_f64(0.75);
    let small_time = cfg.duration - big_time;
    let mut big = CellFanout::new(100_000, 1_000);
    run_cell_fanout(&mut big, cfg.warmup_for(big_time), 0);
    let (big_sets0, big_bytes0) = (big.change_sets(), big.bytes());
    let (latency, ops, elapsed) = run_cell_fanout(&mut big, big_time, cfg.min_ops);
    let (big_sets, big_bytes) = (big.change_sets() - big_sets0, big.bytes() - big_bytes0);
    drop(big);

    let mut small = CellFanout::new(10_000, 1_000);
    run_cell_fanout(&mut small, cfg.warmup_for(small_time), 0);
    let (small_sets0, small_bytes0) = (small.change_sets(), small.bytes());
    let (small_latency, small_ops, _) = run_cell_fanout(&mut small, small_time, cfg.min_ops);
    let (small_sets, small_bytes) = (
        small.change_sets() - small_sets0,
        small.bytes() - small_bytes0,
    );

    let (p50_big, p50_small) = (latency.percentile(0.5), small_latency.percentile(0.5));
    let expected = u32_change_set_bytes(1_000);
    StressReport {
        name: "fanout/sustained",
        ops,
        elapsed,
        warmup: cfg.warmup_for(big_time),
        bytes: big_bytes,
        rss: None,
        invariants: vec![
            Invariant::new(
                format!(
                    "one change-set per transaction ({big_sets} for {ops}; {small_sets} for {small_ops} over 10k)"
                ),
                big_sets == ops && small_sets == small_ops,
            ),
            Invariant::new(
                format!(
                    "{expected} bytes per transaction over 100k and over 10k observed ({} and {})",
                    big_bytes / ops.max(1),
                    small_bytes / small_ops.max(1)
                ),
                big_bytes == ops * expected && small_bytes == small_ops * expected,
            ),
            Invariant::timing(
                format!(
                    "cost follows the dirty count: p50 over 100k observed {p50_big} ns <= 4 x p50 over 10k {p50_small} ns"
                ),
                p50_big <= 4 * p50_small,
            ),
        ],
        notes: vec![format!(
            "10,000 observed: {small_ops} transactions, p50 {p50_small} ns, p99 {} ns",
            small_latency.percentile(0.99)
        )],
        latency,
    }
}

/// **Fan-out across stores** (c'): 1,000 stores of 100 observed signals, one signal written in
/// each, one transaction: 1,000 change-sets that share a transaction, the per-store overhead
/// and the worst case for any platform-side merge.
pub fn fanout_stores(cfg: &StressConfig) -> StressReport {
    let mut fan = StoresFanout::new(1_000, 100);
    let warmup = cfg.warmup();
    if !warmup.is_zero() {
        let deadline = Instant::now() + warmup;
        while Instant::now() < deadline {
            fan.transaction();
        }
    }
    let (sets0, bytes0) = (fan.change_sets(), fan.bytes());
    let mut hist = Histogram::new();
    let mut ops = 0_u64;
    let start = Instant::now();
    let deadline = start + cfg.duration;
    loop {
        let t0 = Instant::now();
        fan.transaction();
        let t1 = Instant::now();
        hist.record(nanos(t1 - t0));
        ops += 1;
        if run_is_over(t1, deadline, ops, cfg.min_ops) {
            break;
        }
    }
    let elapsed = start.elapsed();
    let (sets, bytes) = (fan.change_sets() - sets0, fan.bytes() - bytes0);
    let per_txn = 1_000 * u32_change_set_bytes(1);
    StressReport {
        name: "fanout_stores/sustained",
        ops,
        elapsed,
        warmup,
        latency: hist,
        bytes,
        rss: None,
        invariants: vec![
            Invariant::new(
                format!("1,000 change-sets per transaction, one per store ({sets} for {ops})"),
                sets == ops * 1_000,
            ),
            Invariant::new(
                format!("{per_txn} bytes per transaction ({})", bytes / ops.max(1)),
                bytes == ops * per_txn,
            ),
        ],
        notes: Vec::new(),
    }
}

/// How often the stream scenario samples RSS.
const RSS_EVERY: Duration = Duration::from_millis(250);

/// **Stream backpressure** (d): an always-ready producer and a consumer that grants 16 credits
/// per round, then one that grants 100,000. Undra polls a stream one item ahead of its credit
/// and never further, so whatever the producer could do, memory is bounded: the number of items
/// the producer has made minus the number delivered never exceeds one, and RSS stays flat. The
/// second half measures the item path when credit allows (items per second).
pub fn stream_backpressure(cfg: &StressConfig) -> StressReport {
    let host = Arc::new(CountingHost::default());
    let (rt, producer, call_id) = open_stream(&host);
    let mut series = RssSeries::new();
    let mut max_ahead = 0_u64;
    let mut violations = 0_u64;
    let mut excluded = Duration::ZERO;
    let run_start = Instant::now();
    let mut next_sample = Duration::ZERO;
    let mut sample = |series: &mut RssSeries, excluded: &mut Duration, enabled: bool| {
        if !enabled {
            return;
        }
        let at = run_start.elapsed();
        if at >= next_sample {
            let t0 = Instant::now();
            series.sample(at);
            *excluded += t0.elapsed();
            next_sample = at + RSS_EVERY;
        }
    };
    let mut check = |producer: &Producer, host: &CountingHost| {
        let ahead = producer.produced().saturating_sub(host.stream_items());
        max_ahead = max_ahead.max(ahead);
        if ahead > 1 {
            violations += 1;
        }
    };

    // Half one: a slow consumer, 16 credits a round. At least one round, and `min_ops` items,
    // whatever the clock says: a machine that spends the whole run on the first round still runs
    // the check that the producer is never ahead.
    let half = cfg.duration / 2;
    let mut rounds = 0_u64;
    sample(&mut series, &mut excluded, cfg.rss);
    loop {
        rt.stream_credit(call_id, 16);
        rt.run_pending();
        check(&producer, &host);
        rounds += 1;
        sample(&mut series, &mut excluded, cfg.rss);
        let (now, items) = (run_start.elapsed(), host.stream_items());
        if now >= half && (items >= cfg.min_ops || now >= half + GIVE_UP) {
            break;
        }
    }
    let slow_items = host.stream_items();

    // Half two: a fast consumer, 100,000 credits a round; items per second over the half.
    excluded = Duration::ZERO;
    let fast_start = Instant::now();
    let items_before = host.stream_items();
    let item_bytes_before = host.stream_item_bytes.load(Ordering::Relaxed);
    loop {
        rt.stream_credit(call_id, 100_000);
        rt.run_pending();
        check(&producer, &host);
        sample(&mut series, &mut excluded, cfg.rss);
        // The same rule as the slow half: a round at least, and the floor of items.
        let (now, items) = (run_start.elapsed(), host.stream_items() - items_before);
        if now >= cfg.duration && (items >= cfg.min_ops || now >= cfg.duration + GIVE_UP) {
            break;
        }
    }
    let fast_elapsed = fast_start.elapsed().saturating_sub(excluded);
    let fast_items = host.stream_items() - items_before;
    let fast_bytes = host.stream_item_bytes.load(Ordering::Relaxed) - item_bytes_before;
    series.sample(run_start.elapsed());

    let rss = series.growth(0.2);
    StressReport {
        name: "stream/backpressure",
        ops: fast_items,
        elapsed: fast_elapsed,
        warmup: Duration::ZERO,
        latency: Histogram::new(),
        bytes: fast_bytes,
        rss,
        invariants: vec![
            Invariant::new(
                format!(
                    "the producer is never more than one item ahead of the credit (max {max_ahead}, {violations} violations)"
                ),
                violations == 0 && max_ahead <= 1,
            ),
            Invariant::new(
                format!(
                    "the consumer got every item the producer made, less at most one ({} of {})",
                    host.stream_items(),
                    producer.produced()
                ),
                producer.produced() - host.stream_items() <= 1,
            ),
            warnings(&host),
        ],
        notes: vec![format!(
            "{rounds} rounds of 16 credits delivered {slow_items} items; max ahead {max_ahead}; {} RSS samples",
            series.len()
        )],
    }
}

/// Answers port calls from `COMPLETERS` threads until the host's port queue is closed; returns
/// how many each answered.
pub fn spawn_completers(rt: &Arc<Core>, host: &Arc<DrainHost>) -> Vec<JoinHandle<u64>> {
    (0..COMPLETERS)
        .map(|i| {
            let (rt, host) = (rt.clone(), host.clone());
            std::thread::Builder::new()
                .name(format!("bench-completer-{i}"))
                .spawn(move || {
                    let body = enc(&1_u64);
                    let mut answered = 0_u64;
                    while let Some(id) = host.next_port_call() {
                        rt.port_reply(&port_reply_ok(id, &body));
                        answered += 1;
                    }
                    answered
                })
                .expect("a completer thread starts")
        })
        .collect()
}

/// How long the contended scenario's writer rests between two `call_sync` writes. `ZERO` is a
/// thread that hammers the core lock as fast as it gets it back (it only yields); a short pause is
/// a host thread that writes in bursts, as a UI thread tapping does.
const WRITER_PAUSE: Duration = Duration::ZERO;

/// What the contended scenario's writer thread did.
struct WriterOutcome {
    /// `call_sync` writes made.
    calls: u64,
    /// Of those, the ones that replied `Ok`.
    ok: u64,
    /// Latency of each `call_sync` since the warm-up ended: the wait for the core lock plus the
    /// write.
    latency: Histogram,
    /// `calls` when the warm-up ended.
    warm_calls: u64,
}

/// A host thread writing the `Fetcher` store through `call_sync` (`Fetcher::bump`, adds one to the
/// same signal the completions add one to) until `stop` is set; `made` says how many it has made since
/// the warm-up ended, so the issuer can wait for the writer to have done its share on a machine that
/// starves it.
fn spawn_writer(
    rt: &Arc<Core>,
    bump: Vec<u8>,
    stop: Arc<AtomicBool>,
    warmed: Arc<AtomicBool>,
    made: Arc<AtomicU64>,
) -> JoinHandle<WriterOutcome> {
    let rt = rt.clone();
    std::thread::Builder::new()
        .name("bench-writer".to_owned())
        .spawn(move || {
            let mut out = WriterOutcome {
                calls: 0,
                ok: 0,
                latency: Histogram::new(),
                warm_calls: 0,
            };
            let mut counted_warmup = false;
            while !stop.load(Ordering::Relaxed) {
                if !counted_warmup && warmed.load(Ordering::Relaxed) {
                    // The warm-up is over: what the writer measured so far is not measured.
                    counted_warmup = true;
                    out.warm_calls = out.calls;
                    out.latency.clear();
                }
                let t0 = Instant::now();
                let reply = rt.call_sync(&bump);
                out.latency.record(nanos(t0.elapsed()));
                out.calls += 1;
                if counted_warmup {
                    made.store(out.calls - out.warm_calls, Ordering::Relaxed);
                }
                // `call_id u32, status u8, body`: 0 is Ok.
                if reply.get(4) == Some(&0) {
                    out.ok += 1;
                }
                if WRITER_PAUSE.is_zero() {
                    std::thread::yield_now();
                } else {
                    std::thread::sleep(WRITER_PAUSE);
                }
            }
            out
        })
        .expect("the writer thread starts")
}

/// **Concurrent completions** (e): 8 host threads answering async port calls, a real
/// `undra-core` thread, a "main thread" draining change-sets once a frame (60 Hz) and 256 calls
/// kept in flight. Every call is answered, every answer is one change-set, and the change-sets of
/// the store arrive in increasing transaction order and with the store's total stepping by exactly
/// one each time.
///
/// Only the core thread commits to the store here (a completion runs on it; `port_reply` never
/// takes the core lock), so this proves the completion path under 8 answering threads and not the
/// ordering of commits made from different threads: that is [`completions_contended`].
pub fn completions(cfg: &StressConfig) -> StressReport {
    run_completions(cfg, "completions/8_threads", false)
}

/// **Contended completions** (e'): scenario e plus a host thread that writes the **same store**
/// through `call_sync` as fast as it gets the core lock, so two threads (the `undra-core` thread
/// running the completions, and the writer) commit to one store, interleaved: the core lock
/// serialises the two committers (a task poll and `call_sync` both take it), so this proves the
/// hand-off between them and the delivery under it, not two commits running in parallel, while
/// 8 threads answer port calls and the main thread drains. The total is one signal that every completion and every
/// write adds one to, so the main thread must see exactly `0, 1, 2, .., completions + writes`: any
/// lost, repeated or reordered change-set breaks the sequence, whoever committed it.
pub fn completions_contended(cfg: &StressConfig) -> StressReport {
    run_completions(cfg, "completions/contended", true)
}

fn run_completions(cfg: &StressConfig, name: &'static str, contended: bool) -> StressReport {
    let host = Arc::new(DrainHost::new(4_096, SOURCE_PORT));
    let rt = runtime_with(host.clone(), 1);
    rt.bind_foreign_port(SOURCE_PORT);
    let fetcher = construct(&rt, "Fetcher", &[]);
    rt.observe(fetcher.0, ALL_SIGNALS, true);
    let base_sets = host.counts.change_sets();
    let base_bytes = host.counts.change_set_bytes();

    let stats = Arc::new(MainStats::default());
    let mut main = MainThread::new(stats.clone()).track_sequence(fetcher);
    match cfg.fault {
        Fault::SwapChangeSets => main = main.swap_first_pair(),
        Fault::DropChangeSet => main = main.drop_second_of_first_frame(),
        Fault::None | Fault::SkipPatches | Fault::DropOneUpdate => {}
    }
    let stop = Arc::new(AtomicBool::new(false));
    let main = main.spawn(host.clone(), stop.clone());
    let completers = spawn_completers(&rt, &host);
    let writer_stop = Arc::new(AtomicBool::new(false));
    let warmed = Arc::new(AtomicBool::new(false));
    let writes_made = Arc::new(AtomicU64::new(0));
    let writer = contended.then(|| {
        let bump = method_call(fetcher, "Fetcher", "bump", 3, &[]);
        spawn_writer(
            &rt,
            bump,
            writer_stop.clone(),
            warmed.clone(),
            writes_made.clone(),
        )
    });

    let mut issued = 0_u64;
    let mut rejected = 0_u64;
    let mut call_id = 10_u32;
    // Issues calls until `deadline` has passed, `floor` calls have been issued by this call and,
    // when `writes` is given, the writer thread has made that many writes since the warm-up: a
    // machine that is slow or busy (a writer that is not scheduled for a whole run) takes longer,
    // it does not report less.
    let mut issue_until = |deadline: Instant, floor: u64, writes: Option<u64>| {
        let first = issued;
        loop {
            let now = Instant::now();
            let enough = issued - first >= floor
                && writes.is_none_or(|n| writes_made.load(Ordering::Relaxed) >= n);
            if now >= deadline && (enough || now >= deadline + GIVE_UP) {
                break;
            }
            // The clock is read every `CLOCK_EVERY` calls, not once the window fills: when the
            // completers keep pace the window never fills, and a loop that looks at the clock
            // only then runs as long as it likes (a 200 ms debug run took 154 s under load).
            for _ in 0..CLOCK_EVERY {
                if issued + rejected - host.counts.replies() >= WINDOW {
                    std::thread::yield_now();
                    break;
                }
                call_id += 1;
                host.stamp(call_id);
                let payload =
                    method_call(fetcher, "Fetcher", "fetch", call_id, &enc(&(issued as u32)));
                if rt.call(&payload) == 0 {
                    issued += 1;
                } else {
                    rejected += 1;
                }
            }
        }
    };
    // Warm-up: thread start-up, cold caches, the first calls. What it shipped, answered and timed
    // is not measured; the invariants still cover it (they hold over the whole run).
    let warmup = cfg.warmup();
    if !warmup.is_zero() {
        issue_until(Instant::now() + warmup, 0, None);
    }
    let warm_sets = host.counts.change_sets() - base_sets;
    drop(host.take_latency());
    warmed.store(true, Ordering::Relaxed);

    let start = Instant::now();
    // The contended scenario is about two committers: the writer makes at least `min_ops` writes
    // (one at least) in the measured run as well, whatever its share of the machine.
    let writes_wanted = contended.then(|| cfg.min_ops.max(1));
    issue_until(start + cfg.duration, cfg.min_ops, writes_wanted);
    // The writer stops with the issuer; then let the calls in flight finish.
    writer_stop.store(true, Ordering::Relaxed);
    let written = writer.map(|w| w.join().expect("the writer thread finishes"));
    let patience = Instant::now() + Duration::from_secs(30);
    while host.counts.replies() < issued && Instant::now() < patience {
        std::thread::sleep(Duration::from_millis(1));
    }
    let elapsed = start.elapsed();

    host.close_ports();
    let answered: u64 = completers.into_iter().map(|t| t.join().unwrap_or(0)).sum();
    stop.store(true, Ordering::Release);
    let outcome = main.join().expect("the main-thread model finishes");
    let replies = host.counts.replies();
    let total = rt
        .object::<Fetcher>(fetcher.0)
        .expect("the fetcher")
        .total_now();
    let last_seen = outcome.last_values.first().and_then(|(_, v)| *v);
    let delivered = host.counts.change_sets() - base_sets;
    let shipped = host.counts.change_set_bytes() - base_bytes;
    let order = stats.out_of_order.load(Ordering::Relaxed);
    let steps = stats.sequence_breaks.load(Ordering::Relaxed);
    let walked = stats.change_sets.load(Ordering::Relaxed);
    let largest = stats.largest_batch.load(Ordering::Relaxed);
    let latency = host.latency();
    let (writes, writes_ok) = written.as_ref().map_or((0, 0), |w| (w.calls, w.ok));
    // The store writes of the measured run: change-sets delivered since the warm-up ended.
    let measured_ops = delivered - warm_sets;
    // Every completion and every write adds one to the store's total.
    let expected = issued + writes_ok;

    let mut invariants = vec![
        Invariant::new(
            format!(
                "every call was answered ({replies} replies for {issued} calls; {rejected} rejected)"
            ),
            replies == issued && rejected == 0,
        ),
        Invariant::new(
            format!("the completer threads answered every port call ({answered} of {issued})"),
            answered == issued,
        ),
    ];
    if contended {
        invariants.push(Invariant::new(
            format!(
                "every write from the host thread succeeded ({writes_ok} of {writes} call_sync)"
            ),
            writes == writes_ok && writes > 0,
        ));
    }
    invariants.extend([
        Invariant::new(
            format!(
                "no change-set was lost ({delivered} delivered for {issued} completions and {writes_ok} writes)"
            ),
            delivered == expected,
        ),
        Invariant::new(
            format!(
                "every change-set is one {TICK_BYTES}-byte entry ({shipped} bytes in {delivered})"
            ),
            shipped == TICK_BYTES * delivered,
        ),
        Invariant::new(
            format!(
                "the main thread saw every change-set ({walked} of {})",
                delivered + base_sets
            ),
            walked == delivered + base_sets && stats.malformed.load(Ordering::Relaxed) == 0,
        ),
        Invariant::new(
            format!(
                "nothing arrived out of order ({order} change-sets went backwards in transaction id)"
            ),
            order == 0,
        ),
        Invariant::new(
            format!(
                "the store's total arrived as an exact count, one more each change-set ({steps} steps were not +1)"
            ),
            steps == 0,
        ),
        Invariant::new(
            format!(
                "the store's total is the completions plus the writes ({total} for {issued} + {writes_ok})"
            ),
            total == expected,
        ),
        Invariant::new(
            format!(
                "the last change-set the main thread applied carries that total ({last_seen:?})"
            ),
            last_seen == Some(expected),
        ),
        Invariant::new(
            format!("every reply was Ok ({} were not)", host.reply_errors()),
            host.reply_errors() == 0,
        ),
        warnings(&host.counts),
    ]);

    let mut notes = vec![format!(
        "largest drain {largest} change-sets over {} frames; call to reply p50 {} ns, p99 {} ns",
        stats.frames.load(Ordering::Relaxed),
        latency.percentile(0.5),
        latency.percentile(0.99)
    )];
    if let Some(w) = &written {
        notes.push(format!(
            "writer thread: {} call_sync writes ({:.0}/s), each p50 {} ns, p99 {} ns, max {} ns",
            w.calls - w.warm_calls,
            (w.calls - w.warm_calls) as f64 / elapsed.as_secs_f64(),
            w.latency.percentile(0.5),
            w.latency.percentile(0.99),
            w.latency.max()
        ));
    }
    StressReport {
        name,
        // The operations of this scenario are the writes to the store (each one change-set of
        // `TICK_BYTES`: the invariants above check that exactly): completions and, when
        // contended, the host thread's own.
        ops: measured_ops,
        elapsed,
        warmup,
        bytes: TICK_BYTES * measured_ops,
        rss: None,
        invariants,
        notes,
        latency,
    }
}
