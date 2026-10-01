//! Shutdown hygiene and cancellation on the core (ADR-023; review findings L1, L5 and L6).

mod common;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use common::*;
use parking_lot::Mutex;
use undra_runtime::testing::{RecordingHost, TestRuntime, call_payload, decode_reply};
use undra_runtime::{Host, PortCallOutcome, PortError, Runtime, RuntimeConfig};
use undra_wire::payload::{ReplyStatus, StreamFlag};

const LONG: Duration = Duration::from_secs(60);

fn stat(rt: &Runtime, key: &str) -> u64 {
    let stats: serde_json::Value = serde_json::from_str(&rt.stats_json()).unwrap();
    stats[key].as_u64().unwrap()
}

// ----- L1: shutdown answers everything in flight ------------------------------------------

/// The review's T9: call 40 and stream 41 used to get no terminal message at all.
#[test]
fn l1_shutdown_answers_in_flight_calls_and_ends_open_streams_exactly_once() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 0, "c");
    assert_eq!(t.call(counter_target(h, FOREVER), 40, &[]), 0);
    assert_eq!(t.call(counter_target(h, TICKS), 41, &enc(&5_u32)), 0);
    assert_eq!(t.call(counter_target(h, SLOW_ADD), 42, &enc(&1_i32)), 0);
    t.run_pending();
    let opened = t.take_replies();
    assert_eq!(
        opened.len(),
        1,
        "only the stream opening so far: {opened:?}"
    );
    assert_eq!(opened[0].status, ReplyStatus::StreamOpened);

    t.runtime().shutdown();
    let replies = t.take_replies();
    let mut answered: Vec<(u32, ReplyStatus)> =
        replies.iter().map(|r| (r.call_id, r.status)).collect();
    answered.sort_by_key(|(id, _)| *id);
    assert_eq!(
        answered,
        [(40, ReplyStatus::Cancelled), (42, ReplyStatus::Cancelled)],
        "{replies:?}"
    );
    let items = t.host().take_stream_items();
    assert_eq!(items.len(), 1, "{items:?}");
    // ADR-036: a failed item (flag 3) with status 3, never a String under flag 2.
    assert_eq!((items[0].call_id, items[0].flag), (41, StreamFlag::Failed));
    let failure =
        undra_wire::payload::StreamFailure::decode(&mut undra_wire::Reader::new(&items[0].body))
            .unwrap();
    assert_eq!(failure.status, ReplyStatus::Cancelled);
    assert_eq!(failure.message, "the runtime shut down");
    assert_eq!(failure.detail, "");
    assert_eq!(stat(t.runtime(), "active_calls"), 0);
    assert_eq!(stat(t.runtime(), "tasks"), 0);

    // Idempotent: a second shutdown says nothing more.
    t.runtime().shutdown();
    assert!(t.take_replies().is_empty() && t.host().take_stream_items().is_empty());
}

#[test]
fn l1_pending_port_calls_fail_with_cancelled_and_the_call_waiting_on_one_is_answered() {
    let t = TestRuntime::new();
    t.host().script_port_async(TEST_PORT, ASK);
    let h = new_counter(&t, 0, "c");
    assert_eq!(t.call(counter_target(h, ASK_PORT), 50, &enc(&1_i32)), 0);
    t.run_pending();
    assert_eq!(stat(t.runtime(), "pending_port_calls"), 1);
    t.runtime().shutdown();
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert_eq!(
        (replies[0].call_id, replies[0].status),
        (50, ReplyStatus::Cancelled)
    );
    assert_eq!(stat(t.runtime(), "pending_port_calls"), 0);
}

/// The review's T4: a subscriber that holds a `Ctx` pinned the runtime for good, and `event()` still
/// ran user code after shutdown.
#[test]
fn l1_shutdown_clears_subscribers_and_bindings_so_the_runtime_can_be_freed() {
    let t = TestRuntime::new();
    let rt = t.runtime().clone();
    let weak = Arc::downgrade(&rt);
    let hits = Arc::new(AtomicU32::new(0));
    let (h2, ctx) = (hits.clone(), t.ctx());
    t.ctx()
        .events()
        .subscribe(
            1,
            2,
            Box::new(move |_, _| {
                let _ = &ctx;
                h2.fetch_add(1, Ordering::SeqCst);
            }),
        )
        .detach();
    // A Rust port binding whose fake holds a Ctx is the same kind of cycle.
    struct Fake(#[allow(dead_code)] undra_runtime::Ctx);
    t.ctx().bind_dyn_port::<Fake>(99, Arc::new(Fake(t.ctx())));

    rt.event(1, 2, &[]);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "delivered while the runtime is running"
    );
    drop(t); // TestRuntime::drop shuts the runtime down
    assert!(rt.is_shut_down());
    rt.event(1, 2, &[]);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "no user code runs after shutdown"
    );
    drop(rt);
    assert!(
        weak.upgrade().is_none(),
        "the subscriber and the binding no longer pin the runtime"
    );
}

/// The review's T4b: `spawn`, `sleep` and `port_call` were accepted after shutdown and a spawned
/// task pinned the runtime.
#[test]
fn l1_spawn_sleep_port_call_and_event_after_shutdown_are_logged_no_ops() {
    let t = TestRuntime::new();
    let rt = t.runtime().clone();
    let weak = Arc::downgrade(&rt);
    let ctx = t.ctx();
    rt.shutdown();
    t.host().take_logs();

    // spawn: dropped unpolled, nothing queued, an inert id.
    let dropped = Arc::new(AtomicU32::new(0));
    let ran = Arc::new(AtomicBool::new(false));
    let (keep, d, r) = (ctx.clone(), DropCounter(dropped.clone()), ran.clone());
    let id = ctx.spawn(async move {
        let _keep = keep;
        let _d = d;
        r.store(true, Ordering::SeqCst);
    });
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        1,
        "the future was dropped, unpolled"
    );
    assert_eq!(stat(&rt, "tasks"), 0);
    t.run_pending();
    assert!(!ran.load(Ordering::SeqCst));
    ctx.cancel_task(id); // the inert id is harmless

    // sleep: completes at once, no timer is registered.
    let c2 = ctx.clone();
    t.run_until(async move { c2.sleep(Duration::from_secs(3600)).await });
    assert_eq!(stat(&rt, "pending_timers"), 0);

    // port_call: nothing is sent; the future resolves to Cancelled. Same for the sync form.
    let c3 = ctx.clone();
    let result = t.run_until(async move { c3.port_call(TEST_PORT, ASK, Vec::new()).await });
    assert_eq!(result, Err(PortError::Cancelled));
    assert_eq!(
        ctx.port_call_sync(TEST_PORT, ASK, &[]),
        Err(PortError::Cancelled)
    );
    assert!(t.host().port_calls().is_empty(), "nothing reached the host");

    // event: ignored.
    rt.event(1, 2, &[]);
    let logs = t.host().take_logs();
    for what in ["spawn", "sleep", "port_call", "event"] {
        assert!(
            logs.iter().any(|l| l.level == 3
                && l.message.contains(what)
                && l.message.contains("shut down")),
            "a WARN names {what}: {logs:?}"
        );
    }
    drop((ctx, rt));
    drop(t);
    assert!(weak.upgrade().is_none(), "nothing left pins the runtime");
}

#[test]
fn l1_a_loop_of_late_calls_cannot_flood_the_log() {
    let t = TestRuntime::new();
    let ctx = t.ctx();
    t.runtime().shutdown();
    t.host().take_logs();
    for _ in 0..1000 {
        ctx.spawn(async {});
    }
    let warnings = t
        .host()
        .take_logs()
        .iter()
        .filter(|l| l.level == 3 && l.message.contains("shut down"))
        .count();
    assert!((1..=10).contains(&warnings), "{warnings} warnings");
}

/// A call that slips in around `shutdown` is either refused (status 5, no reply) or answered
/// exactly once (status 3): never silent, never twice.
#[test]
fn l1_calls_racing_shutdown_are_each_answered_exactly_once() {
    for round in 0..25_u32 {
        let host = Arc::new(RecordingHost::new());
        let rt = Runtime::new(
            RuntimeConfig {
                platform: "test".into(),
                log_level: 0,
                ..RuntimeConfig::default()
            },
            host.clone(),
        )
        .unwrap();
        let h = new_counter_rt(&rt, 0, "");
        let hammer = {
            let rt = rt.clone();
            std::thread::spawn(move || {
                let mut accepted = Vec::new();
                for i in 0..300_u32 {
                    let id = round * 10_000 + i + 1;
                    let payload = if i % 3 == 0 {
                        call_payload(counter_target(h, TICKS), id, &enc(&5_u32))
                    } else {
                        call_payload(counter_target(h, FOREVER), id, &[])
                    };
                    if rt.call(&payload) == 0 {
                        accepted.push((id, i % 3 == 0));
                    }
                    if i % 11 == 0 {
                        std::thread::yield_now();
                    }
                }
                accepted
            })
        };
        std::thread::sleep(Duration::from_micros(150 * u64::from(round % 6)));
        with_timeout("shutdown racing calls", LONG, {
            let rt = rt.clone();
            move || rt.shutdown()
        });
        let accepted = hammer.join().unwrap();

        let replies = host.take_replies();
        let items = host.take_stream_items();
        for (id, is_stream) in accepted {
            let terminal_replies = replies
                .iter()
                .filter(|r| r.call_id == id && r.status == ReplyStatus::Cancelled)
                .count();
            let terminal_items = items
                .iter()
                .filter(|i| i.call_id == id && i.flag != StreamFlag::Item)
                .count();
            if is_stream {
                // Opened (status 4) and then ended once; or refused at spawn and cancelled once.
                let opened = replies
                    .iter()
                    .any(|r| r.call_id == id && r.status == ReplyStatus::StreamOpened);
                assert_eq!(
                    terminal_items + terminal_replies,
                    1,
                    "round {round}: stream {id} (opened: {opened}) must end exactly once"
                );
            } else {
                assert_eq!(
                    terminal_replies, 1,
                    "round {round}: call {id} answered exactly once"
                );
            }
        }
        assert_eq!(stat(&rt, "active_calls"), 0, "round {round}");
    }
}

// ----- L5: shutdown from the core or a callback is a contract violation -------------------

/// A host whose `reply` callback shuts the runtime down.
struct ShutdownFromReply {
    rt: OnceLock<Weak<Runtime>>,
    logs: Mutex<Vec<String>>,
}

impl Host for ShutdownFromReply {
    fn reply(&self, _: u32, _: &[u8]) {
        if let Some(rt) = self.rt.get().and_then(Weak::upgrade) {
            rt.shutdown();
        }
    }
    fn change_set(&self, _: &[u8]) {}
    fn stream_item(&self, _: u32, _: &[u8]) {}
    fn port_call(&self, _: u32, _: u32, _: u32, _: &[u8]) -> PortCallOutcome {
        PortCallOutcome::Unavailable
    }
    fn log(&self, _: u8, _: &str, message: &str) {
        self.logs.lock().push(message.to_owned());
    }
}

#[cfg(debug_assertions)]
#[test]
fn l5_shutdown_from_a_host_callback_is_refused_by_a_debug_assertion() {
    let host = Arc::new(ShutdownFromReply {
        rt: OnceLock::new(),
        logs: Mutex::new(Vec::new()),
    });
    let rt = Runtime::new(
        RuntimeConfig {
            core_threads: 0,
            log_level: 0,
            ..RuntimeConfig::default()
        },
        host.clone(),
    )
    .unwrap();
    let _ = host.rt.set(Arc::downgrade(&rt));
    // The reply callback calls shutdown: the assertion panics inside the callback, which the
    // runtime contains (a FATAL log), and the runtime is untouched.
    let payload = call_payload(
        function_target(SUM),
        1,
        &args(|w| {
            use undra_wire::Encode;
            1_i32.encode(w);
            2_i32.encode(w);
        }),
    );
    assert_eq!(rt.call(&payload), 0);
    assert!(!rt.is_shut_down());
    assert!(
        host.logs.lock().iter().any(|l| l
            .contains("Runtime::shutdown was called from the core thread or from a host callback")),
        "{:?}",
        host.logs.lock()
    );
    rt.shutdown();
    assert!(rt.is_shut_down());
}

// ----- L6: cancelled futures are dropped on the core --------------------------------------

/// A future that, when dropped, records whether the dropping thread holds the core lock: it
/// calls back into the runtime, which is refused with `E_REENTRANT` exactly when it does.
struct DropsOnCore {
    rt: Weak<Runtime>,
    outcome: Arc<Mutex<Option<(String, bool)>>>,
}

impl Drop for DropsOnCore {
    fn drop(&mut self) {
        let Some(rt) = self.rt.upgrade() else { return };
        let reply = decode_reply(&rt.call_sync(&call_payload(function_target(SUM), 1, &[])));
        let on_core =
            reply.status == ReplyStatus::BadRequest && reason_of(&reply).contains("E_REENTRANT");
        let name = std::thread::current().name().unwrap_or("").to_owned();
        *self.outcome.lock() = Some((name, on_core));
    }
}

fn threaded_rt() -> (Arc<Runtime>, Arc<RecordingHost>) {
    let host = Arc::new(RecordingHost::new());
    let rt = Runtime::new(
        RuntimeConfig {
            platform: "test".into(),
            log_level: 0,
            ..RuntimeConfig::default()
        },
        host.clone(),
    )
    .unwrap();
    (rt, host)
}

/// `Ctx::cancel_task` dropped the future on the calling thread with no lock, so user `Drop`
/// code ran concurrently with core user code.
#[test]
fn l6_a_cancelled_task_is_dropped_with_the_core_lock_held() {
    let (rt, _host) = threaded_rt();
    let outcome = Arc::new(Mutex::new(None));
    let probe = DropsOnCore {
        rt: Arc::downgrade(&rt),
        outcome: outcome.clone(),
    };
    let started = Arc::new(AtomicBool::new(false));
    let s2 = started.clone();
    let id = rt.ctx().spawn(async move {
        let _probe = probe;
        s2.store(true, Ordering::SeqCst);
        std::future::pending::<()>().await;
    });
    assert!(wait_until(LONG, || started.load(Ordering::SeqCst)));
    rt.ctx().cancel_task(id);
    assert!(
        wait_until(LONG, || outcome.lock().is_some()),
        "the future was dropped"
    );
    let (thread, on_core) = outcome.lock().clone().unwrap();
    assert!(
        on_core,
        "dropped without the core lock, on thread `{thread}`"
    );
    rt.shutdown();
}

/// While the core is busy, `cancel_task` does not wait for it: the drop is queued and happens on
/// the core at the start of its next turn.
#[test]
fn l6_cancel_task_never_waits_for_a_busy_core_and_the_drop_still_happens_on_the_core() {
    let (rt, _host) = threaded_rt();
    let outcome = Arc::new(Mutex::new(None));
    let probe = DropsOnCore {
        rt: Arc::downgrade(&rt),
        outcome: outcome.clone(),
    };
    let started = Arc::new(AtomicBool::new(false));
    let s2 = started.clone();
    let id = rt.ctx().spawn(async move {
        let _probe = probe;
        s2.store(true, Ordering::SeqCst);
        std::future::pending::<()>().await;
    });
    assert!(wait_until(LONG, || started.load(Ordering::SeqCst)));

    // A task that holds the core for 400 ms.
    let busy = Arc::new(AtomicBool::new(false));
    let b2 = busy.clone();
    rt.ctx().spawn(async move {
        b2.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(400));
    });
    assert!(wait_until(LONG, || busy.load(Ordering::SeqCst)));
    let began = std::time::Instant::now();
    rt.ctx().cancel_task(id);
    assert!(
        began.elapsed() < Duration::from_millis(300),
        "cancel_task waited for the core: {:?}",
        began.elapsed()
    );
    assert!(
        outcome.lock().is_none(),
        "not dropped while the core was busy"
    );
    assert!(
        wait_until(LONG, || outcome.lock().is_some()),
        "dropped once the core was free"
    );
    let (thread, on_core) = outcome.lock().clone().unwrap();
    assert!(on_core, "on thread `{thread}`");
    assert_eq!(thread, "undra-core");
    rt.shutdown();
}

#[test]
fn l6_cancel_task_inside_a_task_drops_on_the_spot() {
    let t = TestRuntime::new();
    let dropped = Arc::new(AtomicU32::new(0));
    let ctx = t.ctx();
    let victim = {
        let d = DropCounter(dropped.clone());
        ctx.spawn(async move {
            let _d = d;
            std::future::pending::<()>().await;
        })
    };
    t.run_pending();
    let (c2, d2) = (ctx.clone(), dropped.clone());
    ctx.spawn(async move {
        c2.cancel_task(victim);
        assert_eq!(
            d2.load(Ordering::SeqCst),
            1,
            "dropped before cancel_task returned"
        );
    });
    t.run_pending();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}
