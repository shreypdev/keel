//! The write rule (ADR-023, made a rule of every build by ADR-035): a signal write with
//! consequences is allowed only on a thread that holds **the owning runtime's** core lock, on a
//! `TestRuntime` driver thread or inside `testing::unchecked_writes`; anything else is refused with
//! E0065 before the value changes, and change-sets go to the store's owner.
//!
//! Nothing here depends on the build profile: the CI Rust job runs this file in debug **and** in
//! release (`cargo test --release -p undra-runtime --test write_context`), because the three
//! misbehaviours the gap audit reproduced (OW-1, OW-2) were release-only:
//!
//! 1. a write from a plain thread, on a runtime that is not the global one, was applied and never
//!    delivered (the host and the core disagreed until the next write);
//! 2. a write from a blocking-pool thread was delivered without the core lock, unordered against the
//!    core's transactions;
//! 3. a write to runtime A's store from inside runtime B's core was delivered to B's host under A's
//!    handle.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::*;
use parking_lot::Mutex;
use undra_runtime::testing::{RecordingHost, TestRuntime, unchecked_writes};
use undra_runtime::{Reentrant, Runtime, RuntimeConfig};
use undra_signals::{ALL_SIGNALS, WriteError};
use undra_wire::payload::ReplyStatus;

const E0065: &str = "error[undra::E0065]";
const LONG: Duration = Duration::from_secs(30);

/// A threaded runtime that is not the global one: what every `undra dev` session uses, and what
/// the audit's release probe ran on.
fn threaded() -> (Arc<Runtime>, Arc<RecordingHost>) {
    let host = Arc::new(RecordingHost::new());
    let rt = Runtime::new(
        RuntimeConfig {
            platform: "test".to_owned(),
            mode: "inproc".to_owned(),
            core_threads: 1,
            blocking_threads: 1,
            log_level: 0,
        },
        host.clone(),
    )
    .unwrap();
    (rt, host)
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default()
}

fn stat(rt: &Runtime, key: &str) -> u64 {
    let stats: serde_json::Value = serde_json::from_str(&rt.stats_json()).unwrap();
    stats[key].as_u64().unwrap()
}

/// OW-1 (1): the audit's probe. Release builds applied the write and delivered nothing (change-sets
/// 1 -> 1). Now it is refused, loudly, in every build, before the value changes, and the host hears
/// of the refusal through the owning runtime's log.
#[test]
fn ow1_a_write_from_a_plain_thread_is_refused_and_reported_instead_of_dropped() {
    let (rt, host) = threaded();
    let h = new_counter_rt(&rt, 1, "c");
    rt.observe(h.0, ALL_SIGNALS, true);
    assert_eq!(host.take_change_sets().len(), 1, "the initial values");
    let counter = rt.object::<Counter>(h.0).unwrap();

    let c2 = counter.clone();
    let outcome = std::thread::spawn(move || c2.count.set(2)).join();
    let text = panic_text(&*outcome.expect_err("the write is refused"));
    assert!(
        text.starts_with(&format!(
            "{E0065}: a signal of a store owned by runtime {} was written from a thread that does not hold its core lock",
            rt.id()
        )),
        "{text}"
    );
    assert!(text.contains("ctx.with_core"), "the fix is named: {text}");
    assert_eq!(counter.count.get(), 1, "a refused write changes nothing");
    assert_eq!(host.change_set_count(), 0, "and delivers nothing");
    let logs = host.take_logs();
    assert!(
        logs.iter()
            .any(|l| l.level == undra_runtime::log::ERROR && l.message.contains("E0065")),
        "the owning runtime logged the refusal: {logs:?}"
    );
    assert_eq!(stat(&rt, "off_core_writes"), 1);

    // try_set says so without panicking; can_write answers the question.
    let c3 = counter.clone();
    let owner = rt.id();
    std::thread::spawn(move || {
        assert!(!c3.count.can_write());
        assert_eq!(c3.count.try_set(5), Err(WriteError::OffCore { owner }));
    })
    .join()
    .unwrap();
    assert_eq!(counter.count.get(), 1);
    rt.shutdown();
}

/// OW-1 (2): a pool worker has the runtime installed but not its core lock. Release builds
/// delivered its write without the lock (1 -> 2). Refused in every build.
#[test]
fn ow1_a_write_from_a_blocking_pool_thread_is_refused_in_every_build() {
    let (rt, host) = threaded();
    let h = new_counter_rt(&rt, 1, "c");
    rt.observe(h.0, ALL_SIGNALS, true);
    host.take_change_sets();
    let reply = call_sync_rt(&rt, counter_target(h, BLOCKING_SET), 7, &enc(&5_i32));
    // An async call: the reply arrives through the host.
    assert_eq!(reply.status, ReplyStatus::BadRequest, "{reply:?}");
    assert_eq!(
        rt.call(&counter_call_payload(h, BLOCKING_SET, 8, &enc(&5_i32))),
        0
    );
    assert!(host.wait_for_replies(1, LONG));
    let replies = host.take_replies();
    assert_eq!(replies[0].status, ReplyStatus::Panic, "{replies:?}");
    let message = undra_wire::Reader::new(&replies[0].body)
        .read_str()
        .unwrap()
        .to_owned();
    assert!(message.starts_with(E0065), "{message}");
    let counter = rt.object::<Counter>(h.0).unwrap();
    assert_eq!(counter.count.get(), 1, "nothing was written");
    assert_eq!(host.change_set_count(), 0);
    rt.shutdown();
}

/// OW-2 (3): routing by thread sent runtime A's change-set to runtime B's host, and "holds a core
/// lock" let B's core write A's store at all. The write is refused; `Ctx::with_core` on A is the
/// sanctioned path, and its change-set reaches A's host only.
#[test]
fn ow2_a_write_to_runtime_a_from_runtime_b_is_refused_and_with_core_routes_it_to_a() {
    let (a, host_a) = threaded();
    let (b, host_b) = threaded();
    let h = new_counter_rt(&a, 1, "on a");
    a.observe(h.0, ALL_SIGNALS, true);
    host_a.take_change_sets();
    let counter = a.object::<Counter>(h.0).unwrap();

    // A task on B's core writes A's store.
    let outcome = Arc::new(Mutex::new(None));
    let (c, out) = (counter.clone(), outcome.clone());
    b.spawn(async move {
        let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.count.set(2)));
        *out.lock() = Some(refused.map_err(|p| panic_text(&*p)));
    });
    assert!(wait_until(LONG, || outcome.lock().is_some()));
    let refused = outcome.lock().take().unwrap().unwrap_err();
    assert!(
        refused.contains(&format!("store owned by runtime {}", a.id())),
        "{refused}"
    );
    assert_eq!(counter.count.get(), 1);
    assert_eq!(host_b.change_set_count(), 0, "nothing reached B's host");
    assert_eq!(host_a.change_set_count(), 0);

    // The same write through A's core, from B's task: delivered to A's host, never B's.
    let done = Arc::new(AtomicBool::new(false));
    let (c, ctx_a, flag) = (counter.clone(), a.ctx(), done.clone());
    b.spawn(async move {
        ctx_a
            .with_core(|| c.count.set(3))
            .expect("B's core may enter A");
        flag.store(true, Ordering::SeqCst);
    });
    assert!(wait_until(LONG, || done.load(Ordering::SeqCst)));
    let sets = host_a.take_decoded_change_sets();
    assert_eq!(sets.len(), 1, "exactly one change-set, to the owner");
    assert_eq!(sets[0].entries[0].handle.0, h.0);
    assert_eq!(host_b.change_set_count(), 0);
    assert_eq!(counter.count.get(), 3);
    a.shutdown();
    b.shutdown();
}

#[test]
fn with_core_from_a_host_thread_delivers_exactly_one_change_set() {
    let (rt, host) = threaded();
    let h = new_counter_rt(&rt, 1, "c");
    rt.observe(h.0, ALL_SIGNALS, true);
    host.take_change_sets();
    let counter = rt.object::<Counter>(h.0).unwrap();
    let (ctx, c) = (rt.ctx(), counter.clone());
    let seen = std::thread::spawn(move || {
        ctx.with_core(|| {
            // Two writes, one transaction.
            c.count.set(10);
            c.label.set("ten".to_owned());
            c.count.get()
        })
    })
    .join()
    .unwrap();
    assert_eq!(seen, Ok(10));
    let sets = host.take_decoded_change_sets();
    assert_eq!(sets.len(), 1, "{sets:?}");
    assert_eq!(sets[0].entries.len(), 2);
    rt.shutdown();
}

#[test]
fn with_core_from_a_host_callback_or_the_core_is_e_reentrant() {
    let t = TestRuntime::new();
    // From the core (a dispatched call or a task): the thread holds the lock already.
    let ctx = t.ctx();
    let inner = ctx.clone();
    let from_core = Arc::new(Mutex::new(None));
    let out = from_core.clone();
    t.ctx().spawn(async move {
        *out.lock() = Some(inner.with_core(|| ()));
    });
    t.run_pending();
    assert_eq!(*from_core.lock(), Some(Err(Reentrant)));
    assert!(Reentrant.to_string().starts_with("E_REENTRANT"));

    // From a host callback: the Log port is called with the callback mark set.
    let (rt, host) = threaded();
    let seen = Arc::new(Mutex::new(None));
    let (ctx, out) = (rt.ctx(), seen.clone());
    host.script_port(TEST_PORT, ASK, move |call| {
        *out.lock() = Some(ctx.with_core(|| ()));
        undra_runtime::testing::sync_ok(call, &enc(&1_i32))
    });
    let h = new_counter_rt(&rt, 0, "c");
    assert_eq!(
        rt.call(&counter_call_payload(h, ASK_PORT, 3, &enc(&1_i32))),
        0
    );
    assert!(wait_until(LONG, || seen.lock().is_some()));
    assert_eq!(*seen.lock(), Some(Err(Reentrant)));
    rt.shutdown();
}

/// L4: a test runtime used to run blocking closures inline, on the test thread, so a closure that
/// wrote signals passed in tests and failed natively. Now the closure runs on a pool thread in both,
/// and is refused in both, in every build.
#[test]
fn l4_a_blocking_closure_that_writes_signals_fails_in_a_test_runtime_too() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 1, "c");
    assert_eq!(t.call(counter_target(h, BLOCKING_SET), 7, &enc(&5_i32)), 0);
    t.run_pending();
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert_eq!(replies[0].status, ReplyStatus::Panic, "{replies:?}");
    let message = {
        let mut r = undra_wire::Reader::new(&replies[0].body);
        r.read_str().unwrap().to_owned()
    };
    assert!(message.starts_with(E0065), "{message}");
    assert_eq!(
        decode_body::<i32>(&call_counter(&t, h, GET, 8, &[])),
        1,
        "nothing was written"
    );
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

#[test]
fn l3_the_test_driver_thread_and_the_core_may_write_and_delivery_finds_the_owner() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 1, "c");
    t.runtime().observe(h.0, ALL_SIGNALS, true);
    t.host().take_change_sets();
    let counter = t.runtime().object::<Counter>(h.0).unwrap();
    // The test body (the thread that built the runtime) may write; the change-set goes to the
    // store's owner without entering a scope (ADR-035 routes by owner, not by thread).
    counter.count.set(2);
    assert_eq!(t.host().take_decoded_change_sets().len(), 1);
    // A dispatched call runs on the core.
    assert_eq!(
        decode_body::<i32>(&call_counter(&t, h, ADD, 5, &enc(&3_i32))),
        5
    );
}

/// `unchecked_writes` is the escape hatch for tests that need a write from an arbitrary thread to
/// go through: the write is applied and, routed by owner, delivered to the store's runtime.
#[test]
fn unchecked_writes_lifts_the_check_on_one_thread_only() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 1, "c");
    t.runtime().observe(h.0, ALL_SIGNALS, true);
    t.host().take_change_sets();
    let counter = t.runtime().object::<Counter>(h.0).unwrap();
    let c2 = counter.clone();
    std::thread::spawn(move || unchecked_writes(|| c2.count.set(2)))
        .join()
        .unwrap();
    assert_eq!(counter.count.get(), 2);
    assert_eq!(t.host().take_decoded_change_sets().len(), 1);
    // Another thread is still checked, in every build.
    let c3 = Arc::clone(&counter);
    assert!(std::thread::spawn(move || c3.count.set(3)).join().is_err());
    assert_eq!(counter.count.get(), 2);
}
