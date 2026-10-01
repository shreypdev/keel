//! The write-context check is an allowlist (ADR-023; review findings L3 and L4): in debug builds
//! a signal write is allowed on a thread that holds the core lock, on a `TestRuntime` driver
//! thread and inside `testing::unchecked_writes`, and refused everywhere else.

mod common;

use std::sync::Arc;

use common::*;
use undra_runtime::testing::{TestRuntime, unchecked_writes};
use undra_signals::ALL_SIGNALS;
use undra_wire::payload::ReplyStatus;

const REFUSAL: &str = "not allowed to mutate state";

/// L4: a test runtime used to run blocking closures inline, on the test thread, so a closure that
/// wrote signals passed in tests and panicked in a native debug build. Now the closure runs on a
/// pool thread in both.
#[test]
fn l4_a_blocking_closure_that_writes_signals_fails_in_a_test_runtime_too() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 1, "c");
    assert_eq!(t.call(counter_target(h, BLOCKING_SET), 7, &enc(&5_i32)), 0);
    t.run_pending();
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1, "{replies:?}");
    if cfg!(debug_assertions) {
        assert_eq!(replies[0].status, ReplyStatus::Panic, "{replies:?}");
        let message = {
            let mut r = undra_wire::Reader::new(&replies[0].body);
            r.read_str().unwrap().to_owned()
        };
        assert!(message.contains(REFUSAL), "{message}");
        assert_eq!(
            decode_body::<i32>(&call_counter(&t, h, GET, 8, &[])),
            1,
            "nothing was written"
        );
    } else {
        // Release builds do not evaluate the check: the write went through.
        assert_eq!(replies[0].status, ReplyStatus::Ok);
    }
}

#[test]
fn l4_blocking_closures_of_a_test_runtime_run_on_a_pool_thread_and_the_task_resumes() {
    let t = TestRuntime::new();
    let ctx = t.ctx();
    let name = t.run_until(async move {
        ctx.spawn_blocking(|| std::thread::current().name().unwrap_or("").to_owned())
            .await
    });
    assert!(name.starts_with("undra-blocking-"), "{name}");
    // The calling code sees it as if it had run inline: a call that awaits one is answered once
    // the test has run what is pending.
    let h = new_counter(&t, 0, "c");
    assert_eq!(
        t.call(counter_target(h, BLOCKING_SQUARE), 1, &enc(&12_i32)),
        0
    );
    t.run_pending();
    let replies = t.take_replies();
    assert_eq!(decode_body::<i32>(&replies[0]), 144);
}

/// L3: a write from a thread that is inside no runtime used to be dropped (non-global runtime)
/// or sent to the wrong one, and counted as delivered. It now trips the check.
#[test]
fn l3_a_write_from_an_unscoped_thread_trips_the_checker_in_debug_builds() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 1, "c");
    t.runtime().observe(h.0, ALL_SIGNALS, true);
    t.host().take_change_sets();
    let counter = t.runtime().object::<Counter>(h.0).unwrap();

    let c2 = counter.clone();
    let outcome = std::thread::spawn(move || c2.count.set(2)).join();
    if cfg!(debug_assertions) {
        let payload = outcome.expect_err("the write is refused");
        let text = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_default();
        assert!(text.contains(REFUSAL), "{text}");
        assert_eq!(counter.count.get(), 1, "a refused write changes nothing");
        assert_eq!(t.host().change_set_count(), 0);
    } else {
        outcome.expect("release builds do not check");
    }
}

#[test]
fn l3_the_test_driver_thread_and_the_core_may_write() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 1, "c");
    t.runtime().observe(h.0, ALL_SIGNALS, true);
    t.host().take_change_sets();
    let counter = t.runtime().object::<Counter>(h.0).unwrap();
    // The test body (the thread that built the runtime) may write, in a runtime scope.
    {
        let _scope = t.ctx().enter();
        counter.count.set(2);
    }
    assert_eq!(t.host().take_decoded_change_sets().len(), 1);
    // A dispatched call runs on the core.
    assert_eq!(
        decode_body::<i32>(&call_counter(&t, h, ADD, 5, &enc(&3_i32))),
        5
    );
}

/// `unchecked_writes` is the escape hatch for tests that prove the runtime's lock-level behaviour
/// for release builds: the same off-core write goes through and is delivered.
#[test]
fn unchecked_writes_lifts_the_check_on_one_thread_only() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 1, "c");
    t.runtime().observe(h.0, ALL_SIGNALS, true);
    t.host().take_change_sets();
    let counter = t.runtime().object::<Counter>(h.0).unwrap();
    let ctx = t.ctx();
    let c2 = counter.clone();
    std::thread::spawn(move || {
        let _scope = ctx.enter();
        unchecked_writes(|| c2.count.set(2));
    })
    .join()
    .unwrap();
    assert_eq!(counter.count.get(), 2);
    assert_eq!(t.host().take_decoded_change_sets().len(), 1);
    // Another thread is still checked (debug builds).
    if cfg!(debug_assertions) {
        let c3 = Arc::clone(&counter);
        assert!(std::thread::spawn(move || c3.count.set(3)).join().is_err());
    }
}
