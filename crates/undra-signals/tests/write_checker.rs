//! The write-context check (`set_write_checker`), review finding M2.
//!
//! The checker is process-global, so this file is its own test binary; each test decides what
//! "the core" is for its own thread through a thread-local, so the tests can run in parallel.

mod common;

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;

use common::*;
#[cfg(debug_assertions)]
use undra_signals::Computed;
use undra_signals::{Signal, set_write_checker};

thread_local! {
    /// Whether the current thread pretends to be on the core.
    static ON_CORE: Cell<bool> = const { Cell::new(true) };
}

fn on_core() -> bool {
    ON_CORE.with(Cell::get)
}

fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| set_write_checker(on_core));
}

/// Runs `f` as a thread that is not on the core.
fn off_core<R>(f: impl FnOnce() -> R) -> R {
    ON_CORE.with(|c| c.set(false));
    let result = catch_unwind(AssertUnwindSafe(f));
    ON_CORE.with(|c| c.set(true));
    match result {
        Ok(r) => r,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[test]
#[cfg(debug_assertions)]
fn m2_a_write_from_a_thread_off_the_core_trips_the_checker_in_debug_builds() {
    install();
    let rig = Rig::new();
    let x = Signal::new(0_u32);
    rig.cell.attach(&x, 0).unwrap();
    rig.observe_all();

    // The review's repro: another thread (a blocking-pool worker, a host thread) writes the store.
    let result = catch_unwind(AssertUnwindSafe(|| off_core(|| rig.run(|| x.set(1)))));
    let payload = result.expect_err("the write must be refused in a debug build");
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or_default();
    assert!(
        message.contains("not allowed to mutate state"),
        "unhelpful message: {message}"
    );

    // Refused before anything changed: the value, the transaction state and delivery are intact.
    assert_eq!(x.get(), 0);
    assert!(rig.sets().is_empty());
    rig.run(|| x.set(2));
    assert_eq!(value_of::<u32>(entry(&rig.one_set(), 0)), 2);
}

#[test]
#[cfg(debug_assertions)]
fn m2_a_write_that_reaches_an_attached_computed_is_checked_too() {
    install();
    let rig = Rig::new();
    let local = Signal::new(1_u32); // not attached itself
    let derived = Computed::new(&local, |v: &u32| v * 2);
    rig.cell.attach_computed(&derived, 0).unwrap();
    rig.observe_all();
    let result = catch_unwind(AssertUnwindSafe(|| off_core(|| local.set(5))));
    assert!(result.is_err());
    assert_eq!(local.get(), 1);
}

#[test]
fn m2_a_purely_local_signal_may_be_written_from_anywhere() {
    install();
    // Unattached, and nothing depends on it: there is nothing to deliver and nobody to race.
    let scratch = Signal::new(0_u32);
    off_core(|| scratch.set(9));
    assert_eq!(scratch.get(), 9);
}

#[test]
fn m2_writes_on_the_core_pass() {
    install();
    let rig = Rig::new();
    let x = Signal::new(0_u32);
    rig.cell.attach(&x, 0).unwrap();
    rig.observe_all();
    rig.run(|| x.set(1));
    assert_eq!(rig.one_set().entries.len(), 1);
}

#[test]
#[cfg(not(debug_assertions))]
fn m2_release_builds_never_evaluate_the_checker() {
    install();
    let rig = Rig::new();
    let x = Signal::new(0_u32);
    rig.cell.attach(&x, 0).unwrap();
    rig.observe_all();
    off_core(|| rig.run(|| x.set(1)));
    assert_eq!(x.get(), 1);
}
