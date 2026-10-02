//! ADR-046 decision 4, per containment site (prod-ops review): every panic the runtime contains
//! reaches the `Diagnostics` port once, fire and forget, naming what was running. The calls, async
//! calls and detached tasks are `undra-ports`' `tests/diagnostics.rs`; the computed is
//! `computed_isolation.rs`; these are the other sites a test runtime can reach.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use undra_runtime::background::{BackgroundFuture, BackgroundOutcome};
use undra_runtime::testing::TestRuntime;
use undra_runtime::{DIAGNOSTICS_PORT, InitHook, PANICKED_METHOD};
use undra_wire::Reader;

static HOOK_ARMED: AtomicU32 = AtomicU32::new(0);

undra_meta::inventory::submit! {
    InitHook {
        name: "review.panicking-hook",
        run: |_ctx| {
            if HOOK_ARMED.load(Ordering::SeqCst) != 0 {
                panic!("hook kaboom");
            }
        },
    }
}

/// A test runtime whose `Diagnostics` port is the host's (the platforms' way).
fn rig() -> TestRuntime {
    let t = TestRuntime::new();
    t.runtime().bind_foreign_port(DIAGNOSTICS_PORT);
    t
}

/// `(message, operation)` of every report the host was handed so far; each one fire and forget.
fn reports(t: &TestRuntime) -> Vec<(String, String)> {
    t.host()
        .take_port_calls()
        .into_iter()
        .filter(|c| c.port_id == DIAGNOSTICS_PORT)
        .map(|c| {
            assert_eq!(c.method_id, PANICKED_METHOD);
            assert_eq!(c.port_call_id, 0, "fire and forget");
            let mut r = Reader::new(&c.args);
            let message = r.read_str().unwrap().to_owned();
            let _location = r.read_str().unwrap();
            let operation = r.read_str().unwrap().to_owned();
            (message, operation)
        })
        .collect()
}

fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(m, o)| ((*m).to_owned(), (*o).to_owned()))
        .collect()
}

#[test]
fn a_panicking_init_hook_is_reported_once() {
    let t = rig();
    HOOK_ARMED.store(1, Ordering::SeqCst);
    t.run_init_hooks();
    HOOK_ARMED.store(0, Ordering::SeqCst);
    assert_eq!(
        reports(&t),
        owned(&[("hook kaboom", "init hook review.panicking-hook")])
    );
}

#[test]
fn a_panicking_event_subscriber_is_reported_each_time() {
    const PORT: u32 = undra_meta::ids::port_id("Connectivity");
    const CHANGED: u32 = undra_meta::ids::port_method_id("Connectivity", "changed");
    let t = rig();
    let _bad =
        t.ctx()
            .events()
            .subscribe(PORT, CHANGED, Box::new(|_, _| panic!("subscriber kaboom")));
    t.runtime().event(PORT, CHANGED, &[]);
    t.runtime().event(PORT, CHANGED, &[]);
    assert_eq!(
        reports(&t),
        owned(&[
            ("subscriber kaboom", "event subscriber"),
            ("subscriber kaboom", "event subscriber"),
        ])
    );
}

#[test]
fn a_background_task_that_panics_starting_or_probing_is_reported_and_the_run_goes_on() {
    let t = rig();
    let rt = t.runtime();
    rt.add_background_task(
        "review.bad-start",
        |_| 0,
        |_, _| -> BackgroundFuture { panic!("start kaboom") },
    );
    rt.add_background_task(
        "review.bad-probe",
        |_| panic!("probe kaboom"),
        |_, _| -> BackgroundFuture { Box::pin(async { BackgroundOutcome::Done }) },
    );
    rt.add_background_task(
        "review.bad-run",
        |_| 0,
        |_, _| -> BackgroundFuture { Box::pin(async { panic!("run kaboom") }) },
    );
    let ctx = t.ctx();
    let totals = t.run_until(undra_runtime::background::run(
        &ctx,
        Duration::from_secs(10),
    ));
    assert!(
        !totals.finished,
        "a task that panicked is incomplete: {totals:?}"
    );
    let seen = reports(&t);
    assert_eq!(
        seen,
        owned(&[
            ("start kaboom", "task"),
            ("run kaboom", "task"),
            ("probe kaboom", "background pending review.bad-probe"),
        ]),
        "each contained panic once, in order"
    );
    let stats = rt.stats_json();
    assert!(stats.contains("\"panics\":3"), "{stats}");
    assert!(stats.contains("\"panic_reports\":3"), "{stats}");
    // `stats_json` asks the probes too (its `background.pending`): that panic is one more report.
    assert_eq!(
        reports(&t),
        owned(&[("probe kaboom", "background pending review.bad-probe")])
    );
}

#[test]
fn a_store_whose_restore_panics_is_reported() {
    let t = rig();
    let snapshot = common::snapshot_v2(
        t.runtime(),
        1,
        vec![undra_wire::payload::StoreSnapshot {
            handle: undra_wire::Handle::new(3, 1),
            type_id: common::PANICKY,
            signals: Vec::new(),
        }],
    );
    let restored = t.runtime().restore(&snapshot);
    assert!(restored.is_err(), "{restored:?}");
    let seen = reports(&t);
    assert_eq!(seen.len(), 1, "{seen:?}");
    // (A store the schema does not describe is named by its type id.)
    assert!(seen[0].1.starts_with("restore "), "{seen:?}");
    assert_eq!(seen[0].0, "restore kaboom");
}

#[test]
fn a_report_made_while_one_is_delivered_is_dropped_not_recursed() {
    // The host's port call panics while it is handed a report: the panic is contained (the
    // runtime's own `Host::port_call` guard) and its own report is dropped, not delivered into the
    // port that is panicking, recursively (one FATAL record says what happened).
    let t = rig();
    let calls = Arc::new(AtomicU32::new(0));
    let seen = calls.clone();
    t.host()
        .script_port(DIAGNOSTICS_PORT, PANICKED_METHOD, move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            panic!("the reporter itself panicked")
        });
    let _bad = t.ctx().events().subscribe(
        undra_meta::ids::port_id("Lifecycle"),
        undra_meta::ids::port_method_id("Lifecycle", "changed"),
        Box::new(|_, _| panic!("first")),
    );
    t.runtime().event(
        undra_meta::ids::port_id("Lifecycle"),
        undra_meta::ids::port_method_id("Lifecycle", "changed"),
        &[],
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "handed once, not again");
}
