#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! The soak: a busy app's mixed load, paced, for minutes, looking for leaks and drift.
//!
//! One runtime with a real `keel-core` thread carries, at the same time:
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
//!   64 KiB) from the first sample after the warm-up to the last. The warm-up is the first 20%
//!   of the run but at least 5 s (and at most half the run): the allocator and the threads'
//!   stacks take about 5 s to reach a steady state under this load (RSS climbs 1-3% in that
//!   time on the reference host and is flat after it), and that is warm-up, not growth;
//! * the worst post-warm-up second's p99 is more than 3x the median second's p99 (drift);
//! * an invariant broke: a change-set out of order, a lost completion, the host's copy of the
//!   list different from the core's, a stream more than one item ahead of its credit;
//! * a second's achieved rate was under half its target (the host could not carry the load);
//!   under 95% only warns.
//!
//! `--attempts N` (CI uses 2) runs the soak again when it failed only on the noisy gates (RSS,
//! drift, rate), the way the budgets test takes the best of three; a broken invariant is never
//! retried, because an intermittent reordering is a bug and not noise.
//!
//! `cargo run -p keel-bench --release --bin soak -- --seconds 60` (locally; CI runs 10).
//! Options: `--seconds N`, `--warmup PCT` (default: the rule above), `--rss-limit-pct X`,
//! `--attempts N`, `--json PATH`.
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

use keel::signals::ALL_SIGNALS;
use keel::wire::Encode;
use keel_bench::budget::Budgets;
use keel_bench::rss::{RssSeries, resident_bytes};
use keel_bench::stats::Histogram;

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
  --warmup PCT         the first PCT percent of the run is warm-up (default: 20, but at least 5 s
                       and at most half the run)
  --rss-limit-pct X    RSS growth limit in percent (default: [stress.\"soak/mixed\"] of budgets.toml)
  --attempts N         run again (up to N runs) when only the noisy gates failed: RSS, drift, rate
                       (default 1; a broken invariant is never retried)
  --json PATH          also write one JSON row per second and a summary row (the last attempt)";

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

fn median(sorted: &[u64]) -> u64 {
    sorted[sorted.len() / 2]
}

fn budget_limit() -> f64 {
    let path = match std::env::var_os("KEEL_BENCH_BUDGETS") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("budgets.toml"),
    };
    Budgets::load(&path)
        .ok()
        .and_then(|b| b.stress.get("soak/mixed").and_then(|t| t.rss_growth_pct))
        .unwrap_or(1.0)
}

/// The warm-up as a fraction of the run: what `--warmup` said, else 20% but at least 5 s and at
/// most half the run.
fn warmup_fraction(args: &Args) -> f64 {
    match args.warmup_pct {
        Some(pct) => pct / 100.0,
        None => (5.0 / args.seconds as f64).clamp(0.2, 0.5),
    }
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
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
        .track(fetcher)
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
            } else {
                failures.push(format!("{line}; over the limit of {limit}% (and 64 KiB)"));
            }
        }
        None => notes.push(
            "skip RSS could not be sampled here (no source on this platform, or too few samples): the RSS gate did not run"
                .to_owned(),
        ),
    }

    // Drift.
    let steady: Vec<&Window> = windows
        .iter()
        .filter(|w| w.second as f64 >= warmup * args.seconds as f64 && w.p99_ns > 0)
        .collect();
    if steady.len() >= 2 {
        let mut p99s: Vec<u64> = steady.iter().map(|w| w.p99_ns).collect();
        p99s.sort_unstable();
        let (mid, worst) = (median(&p99s), *p99s.last().unwrap_or(&0));
        let line = format!(
            "firehose p99 per second: median {mid} ns, worst {worst} ns over {} seconds after warm-up",
            steady.len()
        );
        if worst <= 3 * mid {
            notes.push(format!("ok   {line}; limit 3x the median"));
        } else {
            failures.push(format!(
                "{line}: the worst second is more than 3x the median"
            ));
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
    let core_ids = rt
        .object::<Churn>(churn.0)
        .map_err(|e| format!("{e:?}"))?
        .ids();
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
                "the last firehose write is the signal's value ({ticker_now}, seen by the main thread as {:?})",
                tracked(ticker.0)
            ),
            ticker_now == totals.firehose_last_value
                && tracked(ticker.0) == Some(totals.firehose_last_value),
        ),
        (
            format!(
                "the host list equals the core list ({} and {} rows; {} patches applied, {} errors)",
                mirror.map_or(0, |m| m.list.len()),
                core_ids.len(),
                mirror.map_or(0, |m| m.patches),
                mirror.map_or(0, |m| m.errors)
            ),
            mirror.is_some_and(|m| m.errors == 0 && m.ids() == core_ids),
        ),
        (
            format!(
                "the stream is never more than one item ahead of its credit ({ahead} ahead after {delivered_items} of {} credited)",
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
        totals.churn_calls * u64::from(CHURN_PER_CALL),
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

    if let Some(path) = &args.json {
        let all: Vec<String> = failures.iter().chain(&broken).cloned().collect();
        write_json(path, args, warmup, &windows, &series, &notes, &all)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    drop(rt);
    Ok(Outcome {
        passed,
        invariant_broken: !broken.is_empty(),
    })
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One object per second and a last object with the verdict.
fn write_json(
    path: &PathBuf,
    args: &Args,
    warmup: f64,
    windows: &[Window],
    series: &RssSeries,
    notes: &[String],
    failures: &[String],
) -> std::io::Result<()> {
    let mut rows: Vec<String> = windows
        .iter()
        .map(|w| {
            format!(
                "  {{\"second\": {}, \"rss_bytes\": {}, \"p50_ns\": {}, \"p99_ns\": {}, \"p999_ns\": {}, \"firehose_per_sec\": {:.0}, \"churn_ops_per_sec\": {:.0}, \"completions_per_sec\": {:.0}, \"stream_items_per_sec\": {:.0}, \"largest_drain_batch\": {}, \"out_of_order\": {}}}",
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
    let growth = series.growth(warmup);
    rows.push(format!(
        "  {{\"summary\": true, \"seconds\": {}, \"passed\": {}, \"rss_growth_pct\": {}, \"notes\": [{}], \"failures\": [{}]}}",
        args.seconds,
        failures.is_empty(),
        growth.map_or_else(|| "null".to_owned(), |g| format!("{:.3}", g.growth_pct)),
        notes.iter().map(|n| json_string(n)).collect::<Vec<_>>().join(", "),
        failures.iter().map(|n| json_string(n)).collect::<Vec<_>>().join(", "),
    ));
    std::fs::write(path, format!("[\n{}\n]\n", rows.join(",\n")))
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    for attempt in 1..=args.attempts {
        if args.attempts > 1 {
            eprintln!("soak: attempt {attempt} of {}", args.attempts);
        }
        match run(&args) {
            Ok(outcome) if outcome.passed => return ExitCode::SUCCESS,
            Ok(outcome) if outcome.invariant_broken || attempt == args.attempts => {
                return ExitCode::from(1);
            }
            Ok(_) => eprintln!("soak: only the noisy gates failed; trying again"),
            Err(message) => {
                eprintln!("soak: {message}");
                return ExitCode::from(2);
            }
        }
    }
    ExitCode::from(1)
}
