#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! The soak: a busy app's mixed load, paced, for minutes, looking for leaks and drift.
//!
//! One runtime with a real `undra-core` thread carries, at the same time:
//!
//! | Load | Rate | Thread |
//! |---|---|---|
//! | firehose: `Ticker.set`, one transaction each | 100,000 per second | its own, through `call_sync` |
//! | keyed churn: a 10,000-row list, 20 recorded operations per call | 20,000 operations per second | its own |
//! | completions: `Fetcher.fetch` awaiting a port call, 128 in flight | 50,000 per second | its own, plus 8 completer threads |
//! | a stream the consumer keeps up with | 1,000,000 items per second | its own (credit grants) |
//! | the platform's "main thread" draining what was delivered | 60 per second | its own |
//!
//! Every second it samples resident memory, takes the firehose's p99 of that second, and
//! prints one line. It exits non-zero when:
//!
//! * RSS grew by more than the limit (1% of `[stress."soak/mixed"]` in `bench/budgets.toml`, or
//!   64 KiB) from the first sample after the warm-up to the last. The warm-up is the first half
//!   of the run: under this load the allocator and the threads' stacks settle in page-sized steps
//!   for the first 30 s or so (RSS climbs 1-3% on the reference host, then is flat), and that is
//!   warm-up, not growth;
//! * the firehose's p99 **drifts**: a straight line through the per-second p99s after the
//!   warm-up (the Theil-Sen regression of `undra_bench::stats::drift`, which one bad second
//!   cannot tilt) rises by more than half of the median p99 across those seconds, so a tail that
//!   climbs steadily fails, and a p99 that doubles over the second half does too (it needs at
//!   least six seconds after the warm-up: a 10 s run has exactly six). A step that never
//!   comes back (the host's scheduler moving the firehose thread to a slower core for good)
//!   reads as a climb, which is what `--attempts` is for: each attempt is a fresh process;
//! * the worst post-warm-up second's p99 is more than 3x the median second's p99 (the spike
//!   gate, next to the trend: one bad second fails it);
//! * an invariant broke: a change-set out of order, a lost completion, the completions' total not
//!   arriving as an exact count (`0, 1, 2, ..`), the host's copy of the list different from the
//!   core's (field for field, one applied patch per operation), the stream more than one item
//!   ahead of its credit at the end of the run;
//! * a second's achieved rate was under half its target (the host could not carry the load);
//!   under 95% only warns.
//!
//! `--attempts N` (CI uses 2) runs the soak again when it failed only on the noisy gates (RSS,
//! drift, rate), the way the budgets test takes the best of three; a broken invariant is never
//! retried, because an intermittent reordering is a bug and not noise. **Each attempt is a fresh
//! process** (the binary runs itself once per attempt), so attempt 2 starts from a clean heap and
//! a clean scheduler placement instead of inheriting attempt 1's. Exit status: 0 passed, 1 a noisy
//! gate failed (after every attempt), 2 usage or a harness error, 3 an invariant broke.
//!
//! `cargo run -p undra-bench --release --bin soak -- --seconds 60` (locally; CI runs 10).
//! Options: `--seconds N`, `--warmup PCT` (default 50: the first half of the run is warm-up for the
//! RSS and drift gates, which only look at what follows), `--rss-limit-pct X`, `--attempts N`,
//! `--json PATH`. `UNDRA_BENCH_RESULTS_DIR=dir` writes the JSON behind the run (the command, the
//! machine, its load, every second) as `<date>-soak-<N>s.json`. A flat RSS means less on macOS than
//! on Linux: the run prints why (`undra_bench::rss::rss_caveat`).
//!
//! The pieces (`fixtures`, `host`, `stress`) are the ones the sustained scenarios use, included
//! by path so the macro-generated registrations live in this binary and cannot be dropped by
//! the linker.

use std::hint::black_box;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use undra::signals::ALL_SIGNALS;
use undra::wire::Encode;
use undra_bench::budget::Budgets;
use undra_bench::hostinfo;
use undra_bench::results;
use undra_bench::rss::{RssSeries, resident_bytes, rss_caveat};
use undra_bench::stats::{Histogram, drift, median_and_worst};

#[path = "../../common/fixtures.rs"]
mod fixtures;
#[path = "../../common/host.rs"]
mod host;
#[path = "../../common/stress.rs"]
mod stress;

use fixtures::{Churn, Fetcher, Producer, SOURCE_PORT, Ticker};
use host::{DrainHost, ListMirror, MainStats, MainThread, construct, method_call, runtime_with};
use stress::{CHURN_ROWS, nanos, spawn_completers};

/// Firehose writes per second.
const FIREHOSE_RATE: u64 = 100_000;
/// `Churn.churn` calls per second, each of [`CHURN_PER_CALL`] operations.
const CHURN_CALLS_PER_SEC: u64 = 1_000;
/// Operations per `Churn.churn` call: 20 per millisecond is 20,000 operations per second.
const CHURN_PER_CALL: u32 = 20;
/// Completions per second.
const COMPLETION_RATE: u64 = 50_000;
/// Completions kept in flight at most.
const COMPLETION_WINDOW: u64 = 128;
/// Stream items per second.
const STREAM_RATE: u64 = 1_000_000;
/// The call id of the stream; completion call ids start above it.
const STREAM_CALL: u32 = 1;
/// The most the firehose's p99 trend may rise across the seconds after the warm-up, as a fraction
/// of the median p99: half.
const DRIFT_RISE_LIMIT: f64 = 0.5;
/// The fewest seconds after the warm-up a trend is fitted to (fewer say nothing).
const DRIFT_MIN_WINDOWS: usize = 6;
/// The worst second's p99 may be at most this many times the median second's.
const SPIKE_LIMIT: f64 = 3.0;

/// What the command line asked for.
struct Args {
    seconds: u64,
    warmup_pct: Option<f64>,
    rss_limit_pct: Option<f64>,
    attempts: u32,
    json: Option<PathBuf>,
}

const USAGE: &str = "usage: soak [--seconds N] [--warmup PCT] [--rss-limit-pct X] [--attempts N] [--json PATH]

  --seconds N          how long to run (default 60; CI uses 10)
  --warmup PCT         the first PCT percent of the run is warm-up for the RSS and drift gates
                       (default 50: they look at the second half only; the rate gate counts every second)
  --rss-limit-pct X    RSS growth limit in percent (default: [stress.\"soak/mixed\"] of budgets.toml)
  --attempts N         run again (up to N runs, each a fresh process) when only the noisy gates failed:
                       RSS, drift, rate (default 1; a broken invariant is never retried)
  --json PATH          also write the run as JSON: the machine, every second, the verdict (the last attempt)

environment: UNDRA_BENCH_RESULTS_DIR=dir writes <date>-soak-<N>s.json there (UNDRA_BENCH_RESULTS_TAG,
UNDRA_BENCH_DATE adjust the name); UNDRA_BENCH_BUDGETS=path reads another budgets file

exit status: 0 passed; 1 a noisy gate failed; 2 usage or a harness error; 3 an invariant broke";

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        seconds: 60,
        warmup_pct: None,
        rss_limit_pct: None,
        attempts: 1,
        json: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let (name, inline) = match flag.split_once('=') {
            Some((name, value)) => (name.to_owned(), Some(value.to_owned())),
            None => (flag, None),
        };
        if name == "--help" || name == "-h" {
            return Err(USAGE.to_owned());
        }
        let mut value = |what: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| format!("{name} needs {what}\n\n{USAGE}"))
        };
        match name.as_str() {
            "--seconds" => {
                let text = value("a number of seconds")?;
                args.seconds = text.parse().ok().filter(|s| *s >= 2).ok_or_else(|| {
                    format!("--seconds must be a whole number of at least 2, not `{text}`")
                })?;
            }
            "--warmup" => {
                let text = value("a percentage")?;
                args.warmup_pct = Some(
                    text.parse()
                        .ok()
                        .filter(|p: &f64| (0.0..95.0).contains(p))
                        .ok_or_else(|| {
                            format!("--warmup must be a percentage below 95, not `{text}`")
                        })?,
                );
            }
            "--rss-limit-pct" => {
                let text = value("a percentage")?;
                args.rss_limit_pct = Some(
                    text.parse()
                        .ok()
                        .filter(|p: &f64| p.is_finite() && *p >= 0.0)
                        .ok_or_else(|| format!("--rss-limit-pct must be a number, not `{text}`"))?,
                );
            }
            "--attempts" => {
                let text = value("a count")?;
                args.attempts = text
                    .parse()
                    .ok()
                    .filter(|n| (1..=5).contains(n))
                    .ok_or_else(|| format!("--attempts must be 1 to 5, not `{text}`"))?;
            }
            "--json" => args.json = Some(PathBuf::from(value("a path")?)),
            other => return Err(format!("unknown option `{other}`\n\n{USAGE}")),
        }
    }
    Ok(args)
}

/// Hands out the operations a steady rate makes due, at most 50 ms worth at once so a stalled
/// producer does not answer a pause with a storm.
struct Pacer {
    start: Instant,
    per_sec: u64,
    done: u64,
}

impl Pacer {
    fn new(per_sec: u64) -> Pacer {
        Pacer {
            start: Instant::now(),
            per_sec,
            done: 0,
        }
    }

    /// How many operations are due now.
    fn due(&mut self) -> u64 {
        let target =
            (self.start.elapsed().as_nanos() * u128::from(self.per_sec) / 1_000_000_000) as u64;
        let cap = (self.per_sec / 20).max(1);
        if target.saturating_sub(self.done) > cap {
            // Forget the shortfall beyond the cap: the window's achieved rate shows it.
            self.done = target - cap;
        }
        target.saturating_sub(self.done)
    }

    /// Records that `n` of the due operations were made.
    fn did(&mut self, n: u64) {
        self.done += n;
    }

    /// The rest of a one-millisecond slice.
    fn rest(&self) {
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn enc<T: Encode>(value: &T) -> Vec<u8> {
    value.encode_to_vec()
}

/// One second of the run.
#[derive(Clone, Debug)]
struct Window {
    second: u64,
    rss_bytes: Option<u64>,
    p50_ns: u64,
    p99_ns: u64,
    p999_ns: u64,
    firehose_per_sec: f64,
    churn_ops_per_sec: f64,
    completions_per_sec: f64,
    stream_per_sec: f64,
    largest_batch: usize,
    out_of_order: u64,
}

/// What the producer threads report when they stop.
#[derive(Default)]
struct Totals {
    firehose_ops: u64,
    firehose_last_value: u64,
    churn_calls: u64,
    issued: u64,
    rejected: u64,
    credit: u64,
}

/// Shared between the coordinator and the producers.
#[derive(Default)]
struct Live {
    firehose: AtomicU64,
    churn_calls: AtomicU64,
    stop: AtomicBool,
}

fn budget_limit() -> f64 {
    let path = match std::env::var_os("UNDRA_BENCH_BUDGETS") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("budgets.toml"),
    };
    Budgets::load(&path)
        .ok()
        .and_then(|b| b.stress.get("soak/mixed").and_then(|t| t.rss_growth_pct))
        .unwrap_or(1.0)
}

/// The warm-up as a fraction of the run: what `--warmup` said, else the first half.
fn warmup_fraction(args: &Args) -> f64 {
    args.warmup_pct.map_or(0.5, |pct| pct / 100.0)
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Which attempt a child process is (the parent sets `UNDRA_SOAK_ATTEMPT=a/n`).
fn attempt_from_env() -> Option<(u32, u32)> {
    let text = std::env::var("UNDRA_SOAK_ATTEMPT").ok()?;
    let (a, n) = text.split_once('/')?;
    Some((a.parse().ok()?, n.parse().ok()?))
}

/// How a run ended.
struct Outcome {
    passed: bool,
    /// An invariant broke: not retried.
    invariant_broken: bool,
}

fn run(args: &Args) -> Result<Outcome, String> {
    let limit = args.rss_limit_pct.unwrap_or_else(budget_limit);
    let warmup = warmup_fraction(args);
    let load_before = hostinfo::load_average();

    // ---- the runtime and its stores -----------------------------------------------------
    let host = Arc::new(DrainHost::new(4_096, SOURCE_PORT));
    let rt = runtime_with(host.clone(), 1);
    rt.bind_foreign_port(SOURCE_PORT);
    let ticker = construct(&rt, "Ticker", &[]);
    let churn = construct(&rt, "Churn", &[]);
    host::call_ok(
        &rt,
        &method_call(churn, "Churn", "seed", 2, &enc(&CHURN_ROWS)),
    );
    let fetcher = construct(&rt, "Fetcher", &[]);
    let producer = construct(&rt, "Producer", &[]);
    for handle in [ticker, churn, fetcher] {
        rt.observe(handle.0, ALL_SIGNALS, true);
    }
    let producer_object = rt
        .object::<Producer>(producer.0)
        .map_err(|e| format!("{e:?}"))?;
    let stream = method_call(
        producer,
        "Producer",
        "numbers",
        STREAM_CALL,
        &enc(&u64::MAX),
    );
    if rt.call(&stream) != 0 {
        return Err("the stream call was rejected".to_owned());
    }
    let base_replies = host.counts.replies();
    let base_items = host.counts.stream_items();

    // ---- the platform's main thread, and the threads answering port calls -----------------
    let stats = Arc::new(MainStats::default());
    let main_stop = Arc::new(AtomicBool::new(false));
    let main = MainThread::new(stats.clone())
        .mirror(ListMirror::new(churn, 0))
        .track(ticker)
        .track_sequence(fetcher)
        .spawn(host.clone(), main_stop.clone());
    let completers = spawn_completers(&rt, &host);

    // ---- the run ------------------------------------------------------------------------
    eprintln!(
        "soak: {} s, warm-up {:.0}%, RSS limit {}% (or 64 KiB); load: firehose {}/s, churn {}/s, completions {}/s (window {}), stream {}/s, drain 60 Hz",
        args.seconds,
        warmup * 100.0,
        limit,
        FIREHOSE_RATE,
        u64::from(CHURN_PER_CALL) * CHURN_CALLS_PER_SEC,
        COMPLETION_RATE,
        COMPLETION_WINDOW,
        STREAM_RATE,
    );
    if let Some(note) = rss_caveat() {
        eprintln!("note: {note}");
    }
    eprintln!(
        "{:>4} {:>9} {:>7} {:>7} {:>7} {:>10} {:>10} {:>10} {:>10} {:>8} {:>4}",
        "sec",
        "RSS MB",
        "p50 ns",
        "p99 ns",
        "p999 ns",
        "firehose/s",
        "churn/s",
        "compl/s",
        "stream/s",
        "max drain",
        "ooo"
    );

    let live = Live::default();
    let shared = Mutex::new(Histogram::new());
    let mut windows: Vec<Window> = Vec::new();
    let mut series = RssSeries::new();
    let mut notes: Vec<String> = Vec::new();
    let start = Instant::now();

    let totals = std::thread::scope(|scope| {
        let firehose = scope.spawn(|| {
            let payloads: Vec<Vec<u8>> = (0..1024_u64)
                .map(|i| method_call(ticker, "Ticker", "set", 2, &enc(&(i + 1))))
                .collect();
            let mut pacer = Pacer::new(FIREHOSE_RATE);
            let mut local = Histogram::new();
            let mut total = 0_u64;
            while !live.stop.load(Ordering::Relaxed) {
                let due = pacer.due();
                for _ in 0..due {
                    let t0 = Instant::now();
                    let reply = rt.call_sync(&payloads[(total & 1023) as usize]);
                    black_box(&reply);
                    drop(reply);
                    local.record(nanos(t0.elapsed()));
                    total += 1;
                }
                pacer.did(due);
                if local.count() > 0 {
                    shared
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .merge(&local);
                    local.clear();
                }
                live.firehose.store(total, Ordering::Relaxed);
                pacer.rest();
            }
            (
                total,
                if total == 0 {
                    0
                } else {
                    ((total - 1) & 1023) + 1
                },
            )
        });

        let churner = scope.spawn(|| {
            let call = method_call(churn, "Churn", "churn", 3, &enc(&CHURN_PER_CALL));
            let mut pacer = Pacer::new(CHURN_CALLS_PER_SEC);
            let mut calls = 0_u64;
            while !live.stop.load(Ordering::Relaxed) {
                let due = pacer.due();
                for _ in 0..due {
                    black_box(rt.call_sync(&call));
                    calls += 1;
                }
                pacer.did(due);
                live.churn_calls.store(calls, Ordering::Relaxed);
                pacer.rest();
            }
            calls
        });

        let completer = scope.spawn(|| {
            let mut pacer = Pacer::new(COMPLETION_RATE);
            let (mut issued, mut rejected) = (0_u64, 0_u64);
            let mut call_id = 100_u32;
            while !live.stop.load(Ordering::Relaxed) {
                let due = pacer.due();
                let in_flight =
                    (issued + rejected).saturating_sub(host.counts.replies() - base_replies);
                let room = COMPLETION_WINDOW.saturating_sub(in_flight);
                let n = due.min(room);
                for _ in 0..n {
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
                pacer.did(n);
                pacer.rest();
            }
            (issued, rejected)
        });

        let streamer = scope.spawn(|| {
            let mut pacer = Pacer::new(STREAM_RATE);
            let mut credit = 0_u64;
            while !live.stop.load(Ordering::Relaxed) {
                let due = pacer.due();
                if due > 0 {
                    rt.stream_credit(STREAM_CALL, due as u32);
                    credit += due;
                    pacer.did(due);
                }
                pacer.rest();
            }
            credit
        });

        // The coordinator: once a second, sample and print.
        let mut last = (start, 0_u64, 0_u64, 0_u64, 0_u64);
        for second in 1..=args.seconds {
            let due_at = start + Duration::from_secs(second);
            let now = Instant::now();
            if due_at > now {
                std::thread::sleep(due_at - now);
            }
            let now = Instant::now();
            let dt = (now - last.0).as_secs_f64();
            let firehose_ops = live.firehose.load(Ordering::Relaxed);
            let churn_calls = live.churn_calls.load(Ordering::Relaxed);
            let completed = host.counts.replies() - base_replies;
            let streamed = host.counts.stream_items() - base_items;
            let histogram = std::mem::take(&mut *shared.lock().unwrap_or_else(|e| e.into_inner()));
            let rss_bytes = resident_bytes();
            if let Some(bytes) = rss_bytes {
                series.push(now - start, bytes);
            }
            let window = Window {
                second,
                rss_bytes,
                p50_ns: histogram.percentile(0.5),
                p99_ns: histogram.percentile(0.99),
                p999_ns: histogram.percentile(0.999),
                firehose_per_sec: (firehose_ops - last.1) as f64 / dt,
                churn_ops_per_sec: (churn_calls - last.2) as f64 * f64::from(CHURN_PER_CALL) / dt,
                completions_per_sec: (completed - last.3) as f64 / dt,
                stream_per_sec: (streamed - last.4) as f64 / dt,
                largest_batch: stats.largest_batch.swap(0, Ordering::Relaxed),
                out_of_order: stats.out_of_order.load(Ordering::Relaxed),
            };
            last = (now, firehose_ops, churn_calls, completed, streamed);
            eprintln!(
                "{:>4} {:>9} {:>7} {:>7} {:>7} {:>10.0} {:>10.0} {:>10.0} {:>10.0} {:>8} {:>4}",
                window.second,
                window
                    .rss_bytes
                    .map_or_else(|| "-".to_owned(), |b| format!("{:.2}", mb(b))),
                window.p50_ns,
                window.p99_ns,
                window.p999_ns,
                window.firehose_per_sec,
                window.churn_ops_per_sec,
                window.completions_per_sec,
                window.stream_per_sec,
                window.largest_batch,
                window.out_of_order,
            );
            windows.push(window);
        }
        live.stop.store(true, Ordering::Relaxed);
        let (firehose_ops, firehose_last_value) = firehose
            .join()
            .map_err(|_| "the firehose thread panicked".to_owned())?;
        let churn_calls = churner
            .join()
            .map_err(|_| "the churn thread panicked".to_owned())?;
        let (issued, rejected) = completer
            .join()
            .map_err(|_| "the completions thread panicked".to_owned())?;
        let credit = streamer
            .join()
            .map_err(|_| "the stream thread panicked".to_owned())?;
        Ok::<_, String>(Totals {
            firehose_ops,
            firehose_last_value,
            churn_calls,
            issued,
            rejected,
            credit,
        })
    })?;

    // ---- let what is in flight finish, then stop everything -------------------------------
    let patience = Instant::now() + Duration::from_secs(30);
    let finished = |host: &DrainHost| {
        host.counts.replies() - base_replies >= totals.issued
            && host.counts.stream_items() - base_items >= totals.credit
    };
    while !finished(&host) && Instant::now() < patience {
        std::thread::sleep(Duration::from_millis(2));
    }
    host.close_ports();
    let answered: u64 = completers.into_iter().map(|t| t.join().unwrap_or(0)).sum();
    main_stop.store(true, Ordering::Release);
    let outcome = main
        .join()
        .map_err(|_| "the main-thread model panicked".to_owned())?;

    // ---- the verdict ----------------------------------------------------------------------
    let mut failures: Vec<String> = Vec::new();
    // Invariants are not noise: they are not retried.
    let mut broken: Vec<String> = Vec::new();

    // Memory.
    match series.growth(warmup) {
        Some(growth) => {
            let line = format!(
                "RSS {:.2} MB at the first sample after warm-up, {:.2} MB at the end (peak {:.2} MB): {:+.2}%",
                mb(growth.baseline_bytes),
                mb(growth.final_bytes),
                mb(growth.peak_bytes),
                growth.growth_pct
            );
            if growth.within(limit) {
                notes.push(format!("ok   {line}; limit {limit}% or 64 KiB"));
                if rss_caveat().is_some() {
                    notes.push(
                        "note on macOS a flat RSS is weaker evidence than on Linux (see the note at the top)"
                            .to_owned(),
                    );
                }
            } else {
                failures.push(format!("{line}; over the limit of {limit}% (and 64 KiB)"));
            }
        }
        None => notes.push(
            "skip RSS could not be sampled here (no source on this platform, or too few samples): the RSS gate did not run"
                .to_owned(),
        ),
    }

    // Drift: the firehose's p99 over the seconds after the warm-up must neither spike (one bad
    // second against the others) nor climb (the trend through all of them).
    let steady: Vec<&Window> = windows
        .iter()
        .filter(|w| w.second as f64 >= warmup * args.seconds as f64 && w.p99_ns > 0)
        .collect();
    if steady.len() >= 2 {
        let p99s: Vec<f64> = steady.iter().map(|w| w.p99_ns as f64).collect();
        let (mid, worst) = median_and_worst(&p99s).unwrap_or((0.0, 0.0));
        let line = format!(
            "firehose p99 per second: median {mid:.0} ns, worst {worst:.0} ns over {} seconds after warm-up",
            steady.len()
        );
        if worst <= SPIKE_LIMIT * mid {
            notes.push(format!("ok   {line}; limit {SPIKE_LIMIT}x the median"));
        } else {
            failures.push(format!(
                "{line}: the worst second is more than {SPIKE_LIMIT}x the median"
            ));
        }
        let points: Vec<(f64, f64)> = steady
            .iter()
            .map(|w| (w.second as f64, w.p99_ns as f64))
            .collect();
        match drift(&points, DRIFT_MIN_WINDOWS) {
            Some(d) => {
                let line = format!(
                    "firehose p99 trend: {:+.0} ns per second, a rise of {:+.0}% of the median across {} seconds after warm-up",
                    d.slope,
                    d.rise * 100.0,
                    d.points
                );
                if d.exceeds(DRIFT_RISE_LIMIT) {
                    failures.push(format!(
                        "{line}: the p99 is climbing (limit {:+.0}%)",
                        DRIFT_RISE_LIMIT * 100.0
                    ));
                } else {
                    notes.push(format!(
                        "ok   {line}; limit {:+.0}%",
                        DRIFT_RISE_LIMIT * 100.0
                    ));
                }
            }
            None => notes.push(format!(
                "skip p99 trend: fewer than {DRIFT_MIN_WINDOWS} seconds after warm-up (run at least {} s at this warm-up)",
                ((DRIFT_MIN_WINDOWS as f64 / (1.0 - warmup)).ceil() as u64).max(2)
            )),
        }
    } else {
        notes.push("skip p99 drift: fewer than two seconds after warm-up".to_owned());
    }

    // Did the host carry the load?
    let targets = [
        (
            "firehose",
            FIREHOSE_RATE as f64,
            windows
                .iter()
                .map(|w| w.firehose_per_sec)
                .collect::<Vec<_>>(),
        ),
        (
            "churn",
            (u64::from(CHURN_PER_CALL) * CHURN_CALLS_PER_SEC) as f64,
            windows.iter().map(|w| w.churn_ops_per_sec).collect(),
        ),
        (
            "completions",
            COMPLETION_RATE as f64,
            windows.iter().map(|w| w.completions_per_sec).collect(),
        ),
        (
            "stream",
            STREAM_RATE as f64,
            windows.iter().map(|w| w.stream_per_sec).collect(),
        ),
    ];
    for (name, target, rates) in targets {
        let worst = rates.iter().copied().fold(f64::INFINITY, f64::min);
        let mean = rates.iter().sum::<f64>() / rates.len().max(1) as f64;
        let line = format!(
            "{name}: mean {mean:.0}/s of a target of {target:.0}/s; slowest second {worst:.0}/s ({:.0}%)",
            worst / target * 100.0
        );
        if worst < target * 0.5 {
            failures.push(format!("{line}: the host could not carry the load"));
        } else if worst < target * 0.95 {
            notes.push(format!("warn {line}"));
        } else {
            notes.push(format!("ok   {line}"));
        }
    }

    // Invariants.
    let delivered = host.counts.change_sets();
    let walked = stats.change_sets.load(Ordering::Relaxed);
    let out_of_order = stats.out_of_order.load(Ordering::Relaxed);
    let replies = host.counts.replies() - base_replies;
    let core_rows = rt
        .object::<Churn>(churn.0)
        .map_err(|e| format!("{e:?}"))?
        .rows_now();
    let churn_ops = totals.churn_calls * u64::from(CHURN_PER_CALL);
    let total = rt
        .object::<Fetcher>(fetcher.0)
        .map_err(|e| format!("{e:?}"))?
        .total_now();
    let ticker_now = rt
        .object::<Ticker>(ticker.0)
        .map_err(|e| format!("{e:?}"))?
        .current();
    let delivered_items = host.counts.stream_items() - base_items;
    let ahead = producer_object
        .produced()
        .saturating_sub(base_items + delivered_items);
    let mirror = outcome.mirror.as_ref();
    let tracked = |handle: u64| {
        outcome
            .last_values
            .iter()
            .find(|(h, _)| *h == handle)
            .and_then(|(_, v)| *v)
    };
    let checks = [
        (
            format!("nothing arrived out of order ({out_of_order} change-sets went backwards)"),
            out_of_order == 0,
        ),
        (
            format!(
                "the main thread walked every change-set ({walked} of {delivered}; {} malformed)",
                stats.malformed.load(Ordering::Relaxed)
            ),
            walked == delivered && stats.malformed.load(Ordering::Relaxed) == 0,
        ),
        (
            format!(
                "no completion was lost ({replies} replies, {answered} answered, for {} issued; {} rejected)",
                totals.issued, totals.rejected
            ),
            replies == totals.issued && answered == totals.issued && totals.rejected == 0,
        ),
        (
            format!(
                "the store's total is the sum of the completions ({total} for {})",
                totals.issued
            ),
            total == totals.issued && tracked(fetcher.0) == Some(totals.issued),
        ),
        (
            format!(
                "the store's total arrived as an exact count, one more each change-set ({} steps were not +1)",
                stats.sequence_breaks.load(Ordering::Relaxed)
            ),
            stats.sequence_breaks.load(Ordering::Relaxed) == 0,
        ),
        (
            format!(
                "the last firehose write is the signal's value ({ticker_now}, seen by the main thread as {:?})",
                tracked(ticker.0)
            ),
            ticker_now == totals.firehose_last_value
                && tracked(ticker.0) == Some(totals.firehose_last_value),
        ),
        (
            // Rows, not ids (an `Update` never changes an id), and one applied patch per
            // operation (a lost patch can be repaired by a later write of the same row).
            format!(
                "the host list equals the core list, field for field ({} and {} rows; {} patches \
                 applied for {churn_ops} operations, {} full values, {} errors)",
                mirror.map_or(0, |m| m.list.len()),
                core_rows.len(),
                mirror.map_or(0, |m| m.patches),
                mirror.map_or(0, |m| m.fulls),
                mirror.map_or(0, |m| m.errors)
            ),
            mirror.is_some_and(|m| {
                m.errors == 0 && m.list == core_rows && m.patches == churn_ops && m.fulls == 1
            }),
        ),
        (
            format!(
                "at the end, the stream is at most one item ahead of its credit and every credit was used ({ahead} ahead after {delivered_items} of {} credited; checked once, after the run: the every-round check is scenario d's)",
                totals.credit
            ),
            ahead <= 1 && delivered_items == totals.credit,
        ),
        (
            format!(
                "every reply was Ok ({} were not) and nothing warned ({}; the first: {})",
                host.reply_errors(),
                host.counts.warnings(),
                host.counts
                    .first_warning()
                    .unwrap_or_else(|| "none".to_owned())
            ),
            host.reply_errors() == 0 && host.counts.warnings() == 0,
        ),
    ];
    for (line, holds) in checks {
        if holds {
            notes.push(format!("ok   {line}"));
        } else {
            broken.push(line);
        }
    }

    eprintln!();
    eprintln!(
        "totals: {} firehose writes, {} churn operations, {} completions, {} stream items; {delivered} change-sets delivered; largest drain {} change-sets",
        totals.firehose_ops,
        churn_ops,
        totals.issued,
        delivered_items,
        windows.iter().map(|w| w.largest_batch).max().unwrap_or(0),
    );
    for note in &notes {
        eprintln!("  {note}");
    }
    for failure in failures.iter().chain(&broken) {
        eprintln!("  FAIL {failure}");
    }
    let passed = failures.is_empty() && broken.is_empty();
    eprintln!("soak {}", if passed { "PASSED" } else { "FAILED" });

    let all: Vec<String> = failures.iter().chain(&broken).cloned().collect();
    let verdict = Verdict {
        windows: &windows,
        series: &series,
        notes: &notes,
        failures: &all,
        load: (load_before, hostinfo::load_average()),
    };
    if let Some(path) = &args.json {
        results::write(path, &json_text(args, warmup, &verdict));
    }
    if let Some(dir) = results::dir_from_env() {
        let (date, tag) = (hostinfo::date(), results::tag_from_env());
        let path = results::path_for(
            &dir,
            &date,
            tag.as_deref(),
            &format!("soak-{}s", args.seconds),
        );
        results::write(&path, &json_text(args, warmup, &verdict));
        eprintln!("wrote {}", path.display());
    }
    drop(rt);
    Ok(Outcome {
        passed,
        invariant_broken: !broken.is_empty(),
    })
}

/// What a run found, for its JSON.
struct Verdict<'a> {
    windows: &'a [Window],
    series: &'a RssSeries,
    notes: &'a [String],
    failures: &'a [String],
    /// The one-minute load average before and after the run.
    load: (Option<f64>, Option<f64>),
}

/// The run as one JSON object: the command and the machine (with its load), the second-by-second
/// windows and the verdict.
fn json_text(args: &Args, warmup: f64, verdict: &Verdict<'_>) -> String {
    let windows: Vec<String> = verdict
        .windows
        .iter()
        .map(|w| {
            format!(
                "    {{\"second\": {}, \"rss_bytes\": {}, \"p50_ns\": {}, \"p99_ns\": {}, \"p999_ns\": {}, \"firehose_per_sec\": {:.0}, \"churn_ops_per_sec\": {:.0}, \"completions_per_sec\": {:.0}, \"stream_items_per_sec\": {:.0}, \"largest_drain_batch\": {}, \"out_of_order\": {}}}",
                w.second,
                w.rss_bytes.map_or_else(|| "null".to_owned(), |b| b.to_string()),
                w.p50_ns,
                w.p99_ns,
                w.p999_ns,
                w.firehose_per_sec,
                w.churn_ops_per_sec,
                w.completions_per_sec,
                w.stream_per_sec,
                w.largest_batch,
                w.out_of_order,
            )
        })
        .collect();
    let growth = verdict.series.growth(warmup);
    let strings = |items: &[String]| {
        items
            .iter()
            .map(|n| results::json_string(n))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let attempts = attempt_from_env().map_or(args.attempts, |(_, n)| n);
    let mut flags = format!("--seconds {} --attempts {attempts}", args.seconds);
    if let Some(pct) = args.warmup_pct {
        flags.push_str(&format!(" --warmup {pct}"));
    }
    if let Some(pct) = args.rss_limit_pct {
        flags.push_str(&format!(" --rss-limit-pct {pct}"));
    }
    let command = results::command(
        &format!("cargo run -p undra-bench --release --bin soak -- {flags}"),
        &["UNDRA_BENCH_RESULTS_TAG", "UNDRA_BENCH_BUDGETS"],
    );
    let attempt = attempt_from_env().map_or(1, |(a, _)| a);
    format!(
        "{{\n  \"kind\": \"soak\",\n  \"date\": {},\n  \"tag\": {},\n  \"command\": {},\n  {},\n  \
         \"run\": {{\"seconds\": {}, \"warmup_pct\": {}, \"attempt\": {}, \"attempts\": {}, \"rss_caveat\": {}}},\n  \
         \"summary\": {{\"passed\": {}, \"rss_growth_pct\": {}, \"notes\": [{}], \"failures\": [{}]}},\n  \
         \"windows\": [\n{}\n  ]\n}}\n",
        results::json_string(&hostinfo::date()),
        results::tag_from_env().map_or_else(|| "null".to_owned(), |t| results::json_string(&t)),
        results::json_string(&command),
        results::machine_members(verdict.load.0, verdict.load.1),
        args.seconds,
        results::json_number(warmup * 100.0),
        attempt,
        attempts,
        rss_caveat().map_or_else(|| "null".to_owned(), results::json_string),
        verdict.failures.is_empty(),
        growth.map_or_else(|| "null".to_owned(), |g| format!("{:.3}", g.growth_pct)),
        strings(verdict.notes),
        strings(verdict.failures),
        windows.join(",\n"),
    )
}

/// The exit status of one finished run (see the header).
fn exit_status(outcome: &Outcome) -> u8 {
    if outcome.passed {
        0
    } else if outcome.invariant_broken {
        3
    } else {
        1
    }
}

/// The command line of one attempt: this run's options with a single attempt.
fn child_args(args: &Args) -> Vec<String> {
    let mut out = vec![
        "--seconds".to_owned(),
        args.seconds.to_string(),
        "--attempts".to_owned(),
        "1".to_owned(),
    ];
    if let Some(pct) = args.warmup_pct {
        out.extend(["--warmup".to_owned(), pct.to_string()]);
    }
    if let Some(pct) = args.rss_limit_pct {
        out.extend(["--rss-limit-pct".to_owned(), pct.to_string()]);
    }
    if let Some(path) = &args.json {
        out.extend(["--json".to_owned(), path.display().to_string()]);
    }
    out
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    if args.attempts == 1 {
        return match run(&args) {
            Ok(outcome) => ExitCode::from(exit_status(&outcome)),
            Err(message) => {
                eprintln!("soak: {message}");
                ExitCode::from(2)
            }
        };
    }
    // Several attempts: each one is this binary again, in a fresh process, so that attempt 2 does
    // not inherit attempt 1's heap (a leak that started at 9.8 MB started the next attempt at
    // 12.3 MB in review) or its scheduler placement. The exit status says why a child failed:
    // 1 is a noisy gate (try again), 3 an invariant (never retried).
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("soak: cannot find this binary to run the attempts in fresh processes: {e}");
            return ExitCode::from(2);
        }
    };
    for attempt in 1..=args.attempts {
        eprintln!(
            "soak: attempt {attempt} of {} (a fresh process)",
            args.attempts
        );
        let status = std::process::Command::new(&exe)
            .args(child_args(&args))
            .env("UNDRA_SOAK_ATTEMPT", format!("{attempt}/{}", args.attempts))
            .status();
        match status.map(|s| s.code()) {
            Ok(Some(0)) => {
                if attempt > 1 {
                    eprintln!(
                        "soak: passed on attempt {attempt} of {}; the attempts before it failed a \
                         noisy gate (see above)",
                        args.attempts
                    );
                }
                return ExitCode::SUCCESS;
            }
            Ok(Some(3)) => {
                eprintln!("soak: an invariant broke: not retried");
                return ExitCode::from(3);
            }
            Ok(Some(1)) if attempt < args.attempts => {
                eprintln!("soak: only the noisy gates failed; trying again");
            }
            Ok(Some(1)) => return ExitCode::from(1),
            Ok(other) => {
                eprintln!("soak: an attempt ended with {other:?}, not with a verdict");
                return ExitCode::from(2);
            }
            Err(e) => {
                eprintln!("soak: cannot run an attempt: {e}");
                return ExitCode::from(2);
            }
        }
    }
    ExitCode::from(1)
}
