//! The budget gate (constitution R9): every benchmarked operation runs a few thousand times with
//! plain `Instant` timing and its p50 must be under its budget in `bench/budgets.toml`.
//!
//! This is what CI runs (`cargo test -p keel-bench --test budgets --release`); criterion is for
//! humans and stays out of the critical path.
//!
//! * The budgets are **host** regression guards, about five times what an Apple-silicon laptop
//!   measures, not the blueprint's device targets (see `budgets.toml` and `bench/RESULTS.md`).
//! * Without optimisations the numbers mean nothing, so a debug build (plain `cargo test`) only
//!   smoke-runs every operation a few times: fixtures and operations stay working under
//!   `cargo test --workspace`, and the timing assertion is left to `--release`.
//! * A noisy run gets three attempts; the best p50 counts. One slow neighbour does not fail CI,
//!   a real regression is slow on every attempt.
//! * Knobs: `KEEL_BENCH_SCALE=2.5` multiplies every budget (a slower runner), `KEEL_BENCH_FILTER=
//!   keyed` measures only matching names, `KEEL_BENCH_BUDGETS=path` reads another file.
//! * `cargo test -p keel-bench --test budgets --release -- --ignored --nocapture baseline`
//!   prints fresh measurements in `budgets.toml` syntax, for setting or re-basing a budget.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;

use keel_bench::budget::Budgets;
use keel_bench::measure::{MeasureConfig, Stats, measure};
use keel_bench::workload::Workload;

#[path = "../common/mod.rs"]
mod common;

/// Timing tests must not overlap: a second test thread would be noise in the first.
static SERIAL: Mutex<()> = Mutex::new(());

const ATTEMPTS: usize = 3;

fn budgets_path() -> PathBuf {
    match std::env::var_os("KEEL_BENCH_BUDGETS") {
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
    match std::env::var("KEEL_BENCH_SCALE") {
        Ok(text) => match text.parse::<f64>() {
            Ok(scale) if scale.is_finite() && scale > 0.0 => scale,
            _ => panic!("KEEL_BENCH_SCALE must be a positive number, not `{text}`"),
        },
        Err(_) => 1.0,
    }
}

fn selected(workloads: Vec<Workload>) -> Vec<Workload> {
    match std::env::var("KEEL_BENCH_FILTER") {
        Ok(filter) if !filter.is_empty() => workloads
            .into_iter()
            .filter(|w| w.name.contains(&filter))
            .collect(),
        _ => workloads,
    }
}

/// Builds the operation afresh for each attempt and keeps the best p50.
fn best_of(workload: &Workload, attempts: usize, budget_ns: f64) -> Stats {
    let config = MeasureConfig::default();
    let mut best: Option<Stats> = None;
    for _ in 0..attempts {
        let mut op = workload.build();
        let stats = measure(&mut *op, &config);
        if best.is_none_or(|b| stats.p50_ns < b.p50_ns) {
            best = Some(stats);
        }
        if stats.p50_ns <= budget_ns {
            break;
        }
    }
    best.expect("at least one attempt")
}

fn human(ns: f64) -> String {
    if ns >= 1_000_000.0 {
        format!("{:.2} ms", ns / 1_000_000.0)
    } else if ns >= 1_000.0 {
        format!("{:.2} us", ns / 1_000.0)
    } else {
        format!("{ns:.1} ns")
    }
}

#[test]
fn the_budget_file_covers_every_workload_and_nothing_else() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let budgets = load_budgets();
    let workloads: BTreeSet<String> = common::workloads::all()
        .into_iter()
        .map(|w| w.name)
        .collect();
    let budgeted: BTreeSet<String> = budgets.benches.keys().cloned().collect();
    let unbudgeted: Vec<_> = workloads.difference(&budgeted).collect();
    let stale: Vec<_> = budgeted.difference(&workloads).collect();
    assert!(
        unbudgeted.is_empty(),
        "these operations have no budget in budgets.toml (run the `baseline` test and add them): {unbudgeted:?}"
    );
    assert!(
        stale.is_empty(),
        "budgets.toml names operations that no longer exist: {stale:?}"
    );
    for (name, b) in &budgets.benches {
        assert!(b.budget_ns > 0.0, "{name}: a budget of zero can never pass");
        if let Some(measured) = b.measured_ns {
            assert!(
                b.budget_ns >= measured,
                "{name}: the budget ({}) is below what was measured ({measured})",
                b.budget_ns
            );
        }
    }
}

#[test]
fn budgets() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let workloads = selected(common::workloads::all());
    assert!(!workloads.is_empty(), "KEEL_BENCH_FILTER matched nothing");

    if cfg!(debug_assertions) && std::env::var_os("KEEL_BENCH_FORCE").is_none() {
        // Timings from an unoptimised build say nothing; prove the operations still work.
        for workload in &workloads {
            let mut op = workload.build();
            for _ in 0..3 {
                if op.needs_reset() {
                    op.reset();
                }
                op.run();
            }
        }
        eprintln!(
            "budgets: {} operations smoke-run; timing is asserted only with --release \
             (KEEL_BENCH_FORCE=1 to time a debug build anyway)",
            workloads.len()
        );
        return;
    }

    let budgets = load_budgets();
    let scale = scale();
    let mut failures = Vec::new();
    eprintln!(
        "{:<46} {:>10} {:>10} {:>10} {:>7}  blueprint",
        "operation", "p50", "p90", "budget", "margin"
    );
    for workload in &workloads {
        let Some(budget) = budgets.benches.get(&workload.name) else {
            failures.push(format!("{}: no budget in budgets.toml", workload.name));
            continue;
        };
        let limit = budget.budget_ns * scale;
        let stats = best_of(workload, ATTEMPTS, limit);
        let margin = limit / stats.p50_ns;
        let blueprint = match (budget.blueprint_ns, &budget.blueprint) {
            (Some(ns), Some(text)) => {
                format!("{text} (this host: {:.2}x the target)", stats.p50_ns / ns)
            }
            (None, Some(text)) => text.clone(),
            _ => String::new(),
        };
        eprintln!(
            "{:<46} {:>10} {:>10} {:>10} {:>6.1}x  {}",
            workload.name,
            human(stats.p50_ns),
            human(stats.p90_ns),
            human(limit),
            margin,
            blueprint
        );
        if stats.p50_ns > limit {
            failures.push(format!(
                "{}: p50 {} is over its budget of {} ({} iterations, best of up to {ATTEMPTS})",
                workload.name,
                human(stats.p50_ns),
                human(limit),
                stats.iterations
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} budget(s) exceeded:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

/// Not a test: prints measurements as `budgets.toml` tables.
#[test]
#[ignore = "prints a baseline; run it explicitly with --ignored --nocapture"]
fn baseline() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let existing = Budgets::load(&budgets_path()).unwrap_or_default();
    let factor: f64 = std::env::var("KEEL_BENCH_FACTOR")
        .ok()
        .and_then(|f| f.parse().ok())
        .unwrap_or(5.0);
    for workload in selected(common::workloads::all()) {
        let stats = best_of(&workload, 1, f64::MAX);
        let budget = round_up(stats.p50_ns * factor);
        println!("[bench.\"{}\"]", workload.name);
        println!("budget_ns = {budget}");
        println!("measured_ns = {:.1}", stats.p50_ns);
        if let Some(old) = existing.benches.get(&workload.name) {
            if let Some(ns) = old.blueprint_ns {
                println!("blueprint_ns = {ns}");
            }
            if let Some(text) = &old.blueprint {
                println!("blueprint = \"{text}\"");
            }
        }
        println!();
    }
}

/// Two significant figures, rounded up: 1,337 becomes 1,400, so a budget reads as a budget.
fn round_up(ns: f64) -> u64 {
    let ns = ns.max(1.0).ceil() as u64;
    let digits = ns.ilog10();
    if digits < 2 {
        return ns.div_ceil(10) * 10;
    }
    let step = 10_u64.pow(digits - 1);
    ns.div_ceil(step) * step
}
