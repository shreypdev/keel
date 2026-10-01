//! The harsh-conditions gate (constitution R9): each sustained scenario runs for a fixed wall
//! time with its producers, consumers and threads, and is checked against its
//! `[stress."name"]` table in `bench/budgets.toml` and against the invariants it asserts about
//! itself (nothing lost, nothing reordered, the host's list equals the core's list, a stream
//! never more than one item ahead of its credit).
//!
//! This is what CI runs next to the budgets test:
//! `cargo test -p undra-bench --test stress --release -- --nocapture`.
//!
//! * The numbers are the **core side** on a host; the gates are regression guards (throughput
//!   floors at a fifth of what an Apple-silicon laptop measures, tail ceilings at 5x and 10x),
//!   not device targets. `bench/RESULTS.md`, "Harsh conditions", says which is which.
//! * Without optimisations the numbers mean nothing, so a debug build (plain `cargo test`) runs
//!   every scenario for 100 ms and asserts **invariants only**: the scenarios stay working
//!   under `cargo test --workspace`, and the timing gates are left to `--release`.
//! * A noisy run gets three attempts: a scenario passes if any attempt meets every gate. An
//!   invariant that breaks is never retried; it is not noise.
//! * Knobs: `UNDRA_STRESS_SECONDS=10` sets the wall time per scenario (default 2; the numbers in
//!   `RESULTS.md` use 10), `UNDRA_BENCH_SCALE=2.5` divides every floor and multiplies every
//!   ceiling (never bytes, RSS or invariants), `UNDRA_BENCH_FILTER=churn` runs only matching
//!   scenarios, `UNDRA_BENCH_BUDGETS=path` reads another file, `UNDRA_STRESS_JSON=path` also
//!   writes one JSON row per scenario for the site.
//! * `cargo test -p undra-bench --test stress --release -- --ignored --nocapture stress_baseline`
//!   prints fresh measurements as `[stress."name"]` tables, for setting or re-basing a gate.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use undra::wire::payload::ChangeSetBuilder;
use undra::wire::{Handle, Writer};
use undra_bench::budget::{Budgets, StressBudget, StressObserved};

#[path = "../common/mod.rs"]
mod common;

use common::host::OrderChecker;
use common::stress::{BYTES_EXACT, Fault, Scenario, StressConfig, StressReport, scenarios};

/// Timing tests must not overlap: a second test thread would be noise in the first.
static SERIAL: Mutex<()> = Mutex::new(());

const ATTEMPTS: usize = 3;

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn budgets_path() -> PathBuf {
    match std::env::var_os("UNDRA_BENCH_BUDGETS") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("budgets.toml"),
    }
}

fn load_budgets() -> Budgets {
    match Budgets::load(&budgets_path()) {
        Ok(budgets) => budgets,
        Err(e) => panic!("{e}"),
    }
}

fn scale() -> f64 {
    match std::env::var("UNDRA_BENCH_SCALE") {
        Ok(text) => match text.parse::<f64>() {
            Ok(scale) if scale.is_finite() && scale > 0.0 => scale,
            _ => panic!("UNDRA_BENCH_SCALE must be a positive number, not `{text}`"),
        },
        Err(_) => 1.0,
    }
}

/// The wall time of each scenario in release mode.
fn seconds() -> f64 {
    match std::env::var("UNDRA_STRESS_SECONDS") {
        Ok(text) => match text.parse::<f64>() {
            Ok(s) if s.is_finite() && s >= 0.1 => s,
            _ => panic!(
                "UNDRA_STRESS_SECONDS must be a number of seconds, at least 0.1, not `{text}`"
            ),
        },
        Err(_) => 2.0,
    }
}

fn selected() -> Vec<(&'static str, Scenario)> {
    let all = scenarios();
    match std::env::var("UNDRA_BENCH_FILTER") {
        Ok(filter) if !filter.is_empty() => all
            .into_iter()
            .filter(|(name, _)| name.contains(&filter))
            .collect(),
        _ => all,
    }
}

fn is_release() -> bool {
    !cfg!(debug_assertions) || std::env::var_os("UNDRA_BENCH_FORCE").is_some()
}

fn observed(report: &StressReport) -> StressObserved {
    StressObserved {
        per_sec: report.per_sec(),
        p99_ns: report.percentile(0.99).map(|n| n as f64),
        p999_ns: report.percentile(0.999).map(|n| n as f64),
        bytes_per_op: Some(report.bytes_per_op()),
        rss: report.rss,
    }
}

fn human_ns(ns: Option<u64>) -> String {
    match ns {
        None => "-".to_owned(),
        Some(ns) if ns >= 1_000_000 => format!("{:.2} ms", ns as f64 / 1e6),
        Some(ns) if ns >= 1_000 => format!("{:.2} us", ns as f64 / 1e3),
        Some(ns) => format!("{ns} ns"),
    }
}

fn human_rate(per_sec: f64) -> String {
    if per_sec >= 1e6 {
        format!("{:.2} M/s", per_sec / 1e6)
    } else if per_sec >= 1e3 {
        format!("{:.1} k/s", per_sec / 1e3)
    } else {
        format!("{per_sec:.0} /s")
    }
}

fn header() {
    eprintln!(
        "{:<28} {:>11} {:>10} {:>10} {:>10} {:>9} {:>8}  verdict",
        "scenario", "per sec", "p50", "p99", "p999", "bytes/op", "RSS"
    );
}

fn row(report: &StressReport, verdict: &str) {
    let rss = report
        .rss
        .map_or_else(|| "-".to_owned(), |g| format!("{:+.2}%", g.growth_pct));
    eprintln!(
        "{:<28} {:>11} {:>10} {:>10} {:>10} {:>9.1} {:>8}  {verdict}",
        report.name,
        human_rate(report.per_sec()),
        human_ns(report.percentile(0.5)),
        human_ns(report.percentile(0.99)),
        human_ns(report.percentile(0.999)),
        report.bytes_per_op(),
        rss,
    );
    for note in &report.notes {
        eprintln!("    {note}");
    }
}

#[test]
fn stress_table_covers_every_scenario() {
    let _serial = serial();
    let budgets = load_budgets();
    let mut expected: BTreeSet<String> = scenarios().iter().map(|(n, _)| (*n).to_owned()).collect();
    // The soak binary reads its RSS gate from here.
    expected.insert("soak/mixed".to_owned());
    let budgeted: BTreeSet<String> = budgets.stress.keys().cloned().collect();
    let unbudgeted: Vec<_> = expected.difference(&budgeted).collect();
    let stale: Vec<_> = budgeted.difference(&expected).collect();
    assert!(
        unbudgeted.is_empty(),
        "these scenarios have no [stress.\"...\"] table in budgets.toml (run `stress_baseline` and add them): {unbudgeted:?}"
    );
    assert!(
        stale.is_empty(),
        "budgets.toml has stress tables for scenarios that do not exist: {stale:?}"
    );
    for (name, table) in &budgets.stress {
        let problems = table.self_check();
        assert!(problems.is_empty(), "{name}: {problems:?}");
    }
}

#[test]
fn stress() {
    let _serial = serial();
    let scenarios = selected();
    assert!(!scenarios.is_empty(), "UNDRA_BENCH_FILTER matched nothing");

    if !is_release() {
        // Timings from an unoptimised build say nothing; prove the scenarios still work and
        // that every invariant holds.
        let cfg = StressConfig {
            duration: Duration::from_millis(100),
            rss: false,
            fault: Fault::None,
        };
        for (name, run) in &scenarios {
            let report = run(&cfg);
            assert_eq!(report.name, *name);
            assert!(report.ops > 0, "{name}: nothing ran");
            let broken = report.broken();
            assert!(
                broken.is_empty(),
                "{name}: {}",
                broken
                    .iter()
                    .map(|i| i.what.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
        eprintln!(
            "stress: {} scenarios smoke-run, invariants only; timing is asserted only with \
             --release (UNDRA_BENCH_FORCE=1 to time a debug build anyway)",
            scenarios.len()
        );
        return;
    }

    let budgets = load_budgets();
    let scale = scale();
    let cfg = StressConfig::new(Duration::from_secs_f64(seconds()));
    let mut failures = Vec::new();
    let mut final_reports = Vec::new();
    eprintln!(
        "{} s per scenario, scale {scale}; RSS is sampled {}",
        seconds(),
        if undra_bench::rss::resident_bytes().is_some() {
            "from the process"
        } else {
            "NOWHERE on this platform (the RSS gates are skipped)"
        }
    );
    header();
    for (name, run) in &scenarios {
        let Some(budget) = budgets.stress.get(*name) else {
            failures.push(format!(
                "{name}: no [stress.\"{name}\"] table in budgets.toml"
            ));
            continue;
        };
        for attempt in 1..=ATTEMPTS {
            let report = run(&cfg);
            let broken = report.broken();
            if !broken.is_empty() {
                row(&report, "INVARIANT BROKEN");
                for invariant in &broken {
                    eprintln!("    x {}", invariant.what);
                    failures.push(format!("{name}: {}", invariant.what));
                }
                break;
            }
            let verdict = budget.check(&observed(&report), scale);
            for notice in &verdict.notices {
                eprintln!("    notice: {notice}");
            }
            if verdict.passed() {
                let tag = if attempt == 1 {
                    "ok".to_owned()
                } else {
                    format!("ok (attempt {attempt})")
                };
                row(&report, &tag);
                final_reports.push(report);
                break;
            }
            row(
                &report,
                &format!("OVER a gate (attempt {attempt} of {ATTEMPTS})"),
            );
            for problem in &verdict.failures {
                eprintln!("    x {problem}");
            }
            if attempt == ATTEMPTS {
                for problem in verdict.failures {
                    failures.push(format!("{name}: {problem}"));
                }
            }
        }
    }
    if let Some(path) = std::env::var_os("UNDRA_STRESS_JSON") {
        write_json(&PathBuf::from(path), &final_reports, &budgets);
    }
    assert!(
        failures.is_empty(),
        "{} stress gate(s) failed:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

#[test]
fn a_skipped_patch_fails_the_equality_invariant() {
    let _serial = serial();
    let cfg = StressConfig {
        duration: Duration::from_millis(200),
        rss: false,
        fault: Fault::SkipPatches,
    };
    let report = common::stress::keyed_churn(&cfg);
    let broken: Vec<_> = report.broken().iter().map(|i| i.what.clone()).collect();
    assert!(
        broken
            .iter()
            .any(|what| what.contains("the host list equals the core list")),
        "a host that drops a patch must fail the equality invariant: {broken:?}"
    );
}

#[test]
fn swapped_change_sets_fail_the_order_invariant() {
    let _serial = serial();
    let cfg = StressConfig {
        duration: Duration::from_millis(200),
        rss: false,
        fault: Fault::SwapChangeSets,
    };
    let report = common::stress::completions(&cfg);
    let broken: Vec<_> = report.broken().iter().map(|i| i.what.clone()).collect();
    assert!(
        broken
            .iter()
            .any(|what| what.contains("nothing arrived out of order")),
        "a main thread that swaps two change-sets must fail the order invariant: {broken:?}"
    );
    // Only that: nothing was lost and every call was answered.
    assert_eq!(broken.len(), 1, "{broken:?}");
}

/// A change-set of one entry for store `handle`, transaction `txn_id`.
fn change_set(txn_id: u64, handles: &[u32]) -> Vec<u8> {
    let mut w = Writer::new();
    let mut b = ChangeSetBuilder::new(&mut w, txn_id);
    for handle in handles {
        b.push(
            Handle::new(*handle, 1),
            0,
            undra::wire::payload::ChangeOp::Full,
            &[0; 8],
        );
    }
    b.finish();
    w.into_vec()
}

#[test]
fn the_order_checker_accepts_increasing_ids_per_store_and_rejects_anything_else() {
    let mut ok = OrderChecker::default();
    // Two stores interleave freely: only each store's own sequence matters.
    for (txn, stores) in [(1, &[1][..]), (2, &[2]), (3, &[1, 2]), (9, &[1])] {
        assert!(ok.check(&change_set(txn, stores)), "txn {txn}");
    }
    // A repeat, a step back, and a swapped pair all fail.
    assert!(!ok.check(&change_set(9, &[1])), "the same id twice");
    assert!(
        !ok.check(&change_set(2, &[2])),
        "a step back for store 2 (it was at 3)"
    );
    let mut swapped = OrderChecker::default();
    assert!(
        swapped.check(&change_set(6, &[7])),
        "the first change-set of a store has nothing to follow"
    );
    assert!(!swapped.check(&change_set(5, &[7])), "a swapped pair");
    let mut garbage = OrderChecker::default();
    assert!(
        !garbage.check(&[1, 2, 3]),
        "a malformed change-set is not in order"
    );
}

// ---------------------------------------------------------------------------------------------
// Baseline and the site's JSON
// ---------------------------------------------------------------------------------------------

/// Two significant figures, rounded up: 1,337 becomes 1,400.
fn round_up(value: f64) -> u64 {
    let n = value.max(1.0).ceil() as u64;
    let digits = n.ilog10();
    if digits < 2 {
        return n.div_ceil(10) * 10;
    }
    let step = 10_u64.pow(digits - 1);
    n.div_ceil(step) * step
}

/// Two significant figures, rounded down: 5,799,209 becomes 5,700,000.
fn round_down(value: f64) -> u64 {
    let n = value.max(1.0).floor() as u64;
    let digits = n.ilog10();
    if digits < 2 {
        return n;
    }
    let step = 10_u64.pow(digits - 1);
    n / step * step
}

/// Not a test: prints measurements as `budgets.toml` tables.
#[test]
#[ignore = "prints a baseline; run it explicitly with --ignored --nocapture"]
fn stress_baseline() {
    let _serial = serial();
    let cfg = StressConfig::new(Duration::from_secs_f64(seconds()));
    let factor: f64 = std::env::var("UNDRA_BENCH_FACTOR")
        .ok()
        .and_then(|f| f.parse().ok())
        .unwrap_or(5.0);
    for (name, run) in selected() {
        let report = run(&cfg);
        let per_sec = report.per_sec();
        println!("[stress.\"{name}\"]");
        println!("min_per_sec = {}", round_down(per_sec / factor));
        if let (Some(p99), Some(p999)) = (report.percentile(0.99), report.percentile(0.999)) {
            println!("p99_ns = {}", round_up(p99 as f64 * factor));
            println!("p999_ns = {}", round_up(p999 as f64 * factor * 2.0));
        }
        let bytes = report.bytes_per_op();
        if name != "stream/backpressure" {
            if BYTES_EXACT.contains(&name) {
                println!("bytes_per_op = {}", bytes.ceil() as u64);
            } else {
                println!(
                    "bytes_per_op = {}  # 1.1x the measured {bytes:.1}",
                    (bytes * 1.1).ceil() as u64
                );
            }
        }
        if let Some(growth) = report.rss {
            println!("rss_growth_pct = 1");
            println!(
                "# RSS {:+.2}% after warm-up ({} to {} bytes)",
                growth.growth_pct, growth.baseline_bytes, growth.final_bytes
            );
        }
        println!("measured_per_sec = {}", per_sec.round() as u64);
        if let (Some(p99), Some(p999)) = (report.percentile(0.99), report.percentile(0.999)) {
            println!("measured_p99_ns = {p99}");
            println!("measured_p999_ns = {p999}");
        }
        if name != "stream/backpressure" {
            println!("measured_bytes_per_op = {bytes:.1}");
        }
        for note in &report.notes {
            println!("# {note}");
        }
        for invariant in report.broken() {
            println!("# INVARIANT BROKEN: {}", invariant.what);
        }
        println!();
    }
}

/// What the site shows for a scenario: its id, label, the unit of one operation and the step that
/// is timed.
const SITE_ROWS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "firehose/sustained",
        "stress/firehose",
        "Firehose: one signal, one transaction per update",
        "txn",
        "transaction (commit + deliver)",
    ),
    (
        "event/sustained",
        "stress/event",
        "Event firehose: host events written into a signal",
        "events",
        "event (commit + deliver)",
    ),
    (
        "keyed_churn_10k/sustained",
        "stress/keyed_churn",
        "Keyed churn: a 10,000-row list, one list operation per transaction",
        "ops",
        "operation (commit + deliver + host apply)",
    ),
    (
        "fanout/sustained",
        "stress/fanout",
        "Fan-out: 1,000 of 100,000 observed signals changed in one transaction",
        "txn",
        "transaction",
    ),
    (
        "fanout_stores/sustained",
        "stress/fanout_stores",
        "Fan-out across 1,000 stores in one transaction",
        "txn",
        "transaction of 1,000 change-sets",
    ),
    (
        "stream/backpressure",
        "stress/stream",
        "Stream backpressure: never more than one item ahead of the consumer",
        "items",
        "",
    ),
    (
        "completions/8_threads",
        "stress/completions",
        "Concurrent completions: 8 threads, nothing lost or reordered",
        "completions",
        "call to reply",
    ),
];

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

fn human_count(per_sec: f64, unit: &str) -> String {
    if per_sec >= 1e6 {
        format!("{:.1} M {unit}/s", per_sec / 1e6)
    } else if per_sec >= 1e3 {
        format!("{:.0} k {unit}/s", per_sec / 1e3)
    } else {
        format!("{per_sec:.0} {unit}/s")
    }
}

fn gate_text(budget: &StressBudget, unit: &str) -> String {
    let mut parts = Vec::new();
    if let Some(floor) = budget.min_per_sec {
        parts.push(format!("at least {}", human_count(floor, unit)));
    }
    if let Some(p99) = budget.p99_ns {
        parts.push(format!("p99 at most {}", human_ns(Some(p99 as u64))));
    }
    if let Some(pct) = budget.rss_growth_pct {
        parts.push(format!("RSS growth at most {pct}%"));
    }
    format!("CI: {}", parts.join(", "))
}

/// Writes one JSON row per scenario in the shape of `site/data/bench.json`'s "harsh" group.
fn write_json(path: &PathBuf, reports: &[StressReport], budgets: &Budgets) {
    let machine = budgets.meta.get("machine").cloned().unwrap_or_default();
    let mut rows = Vec::new();
    for report in reports {
        let Some((_, id, label, unit, step)) = SITE_ROWS.iter().find(|r| r.0 == report.name) else {
            continue;
        };
        let detail = match (report.percentile(0.99), report.name) {
            (Some(p99), _) if !step.is_empty() => format!(
                "p99 {} per {step}, {:.0} bytes each; every invariant held",
                human_ns(Some(p99)),
                report.bytes_per_op()
            ),
            _ => report.notes.first().cloned().unwrap_or_default(),
        };
        let gate = budgets
            .stress
            .get(report.name)
            .map(|b| gate_text(b, unit))
            .unwrap_or_default();
        rows.push(format!(
            "  {{\n    \"group\": \"harsh\",\n    \"id\": {},\n    \"label\": {},\n    \"value\": {},\n    \"detail\": {},\n    \"gate\": {},\n    \"where\": {},\n    \"source\": \"bench/RESULTS.md#harsh-conditions\"\n  }}",
            json_string(id),
            json_string(label),
            json_string(&human_count(report.per_sec(), unit)),
            json_string(&detail),
            json_string(&gate),
            json_string(&format!("{machine}, core only (host)")),
        ));
    }
    let text = format!("[\n{}\n]\n", rows.join(",\n"));
    if let Err(e) = std::fs::write(path, text) {
        panic!("cannot write UNDRA_STRESS_JSON to {}: {e}", path.display());
    }
    eprintln!("wrote {} rows to {}", rows.len(), path.display());
}
