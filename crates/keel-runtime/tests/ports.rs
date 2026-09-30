//! Ports: async and sync port calls, unavailable ports, Rust bindings, and events.

mod common;

use std::any::Any;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use common::*;
use keel_runtime::testing::{
    HostEvent, PortCallRecord, TestRuntime, port_reply, port_reply_ok, sync_ok,
};
use keel_runtime::{
    Host, PortCallOutcome, PortDispatch, PortDispatcher, PortError, Runtime, RuntimeConfig,
};
use keel_wire::payload::{PortStatus, ReplyStatus};

fn ask(t: &TestRuntime, handle: keel_runtime::Handle, x: i32, call_id: u32) {
    assert_eq!(
        t.call(counter_target(handle, ASK_PORT), call_id, &enc(&x)),
        0
    );
}

// ----- async port calls -------------------------------------------------------------------

#[test]
fn async_port_call_round_trips_through_port_reply() {
    let t = TestRuntime::new();
    t.host().script_port_async(TEST_PORT, ASK);
    let h = new_counter(&t, 0, "");
    ask(&t, h, 21, 5);
    t.run_pending();

    // The runtime called the host, and the call is still open.
    let calls = t.host().port_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!((calls[0].port_id, calls[0].method_id), (TEST_PORT, ASK));
    assert_eq!(calls[0].args, enc(&21_i32));
    assert_ne!(calls[0].port_call_id, 0);
    assert!(t.take_replies().is_empty());

    t.runtime()
        .port_reply(&port_reply_ok(calls[0].port_call_id, &enc(&42_i32)));
    assert!(
        t.take_replies().is_empty(),
        "the task runs on the next turn, not inside port_reply"
    );
    t.run_pending();
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].call_id, 5);
    assert_eq!(decode_body::<i32>(&replies[0]), 42);
    assert_eq!(
        t.host()
            .take_timeline()
            .into_iter()
            .filter(|e| !matches!(e, HostEvent::Reply(_)))
            .count(),
        1,
        "exactly one port call"
    );
}

#[test]
fn a_reply_before_the_task_is_polled_again_is_not_lost() {
    // The reply arrives while the task is not being polled (parked): the wake queues it.
    let t = TestRuntime::new();
    t.host().script_port_async(TEST_PORT, ASK);
    let h = new_counter(&t, 0, "");
    ask(&t, h, 1, 1);
    t.run_pending();
    let id = t.host().port_calls()[0].port_call_id;
    t.runtime().port_reply(&port_reply_ok(id, &enc(&5_i32)));
    t.run_pending();
    assert_eq!(decode_body::<i32>(&t.take_replies()[0]), 5);
}

#[test]
fn concurrent_port_calls_are_matched_by_id_in_any_order() {
    let t = TestRuntime::new();
    t.host().script_port_async(TEST_PORT, ASK);
    let h = new_counter(&t, 0, "");
    for call_id in 1..=3 {
        ask(&t, h, call_id as i32 * 10, call_id);
    }
    t.run_pending();
    let calls = t.host().port_calls();
    assert_eq!(calls.len(), 3);
    let mut ids: Vec<u32> = calls.iter().map(|c| c.port_call_id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        3,
        "port_call_ids are unique among in-flight calls"
    );

    // Answer in reverse order; each reply carries its own call's argument back.
    for call in calls.iter().rev() {
        let x: i32 = decode_from(&call.args);
        t.runtime()
            .port_reply(&port_reply_ok(call.port_call_id, &enc(&(x + 1))));
        t.run_pending();
    }
    let replies = t.take_replies();
    let got: Vec<(u32, i32)> = replies
        .iter()
        .map(|r| (r.call_id, decode_body(r)))
        .collect();
    assert_eq!(got, [(3, 31), (2, 21), (1, 11)]);
}

fn decode_from<T: keel_wire::Decode>(bytes: &[u8]) -> T {
    T::decode(&mut keel_wire::Reader::new(bytes)).unwrap()
}

#[test]
fn a_sync_outcome_completes_without_a_port_reply() {
    let t = TestRuntime::new();
    t.host()
        .script_port(TEST_PORT, ASK, |call: &PortCallRecord| {
            let x: i32 = decode_from(&call.args);
            sync_ok(call, &enc(&(x * 2)))
        });
    let h = new_counter(&t, 0, "");
    ask(&t, h, 21, 7);
    t.run_pending();
    assert_eq!(decode_body::<i32>(&t.take_replies()[0]), 42);
    assert!(t.host().port_calls().len() == 1);
    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    assert_eq!(stats["pending_port_calls"], 0);
}

#[test]
fn a_port_error_arrives_as_the_typed_error_bytes() {
    let t = TestRuntime::new();
    t.host().script_port_async(TEST_PORT, ASK);
    let h = new_counter(&t, 0, "");
    ask(&t, h, 1, 1);
    t.run_pending();
    let id = t.host().port_calls()[0].port_call_id;
    t.runtime()
        .port_reply(&port_reply(id, PortStatus::Error, &enc(&503_u16)));
    t.run_pending();
    let reply = t.take_replies().remove(0);
    assert_eq!(reply.status, ReplyStatus::Error);
    assert_eq!(reply.body, enc(&503_u16));
}

#[test]
fn unavailable_ports_fail_the_call_instead_of_hanging() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 0, "");
    // Nothing scripted: the host answers `Unavailable`.
    ask(&t, h, 1, 1);
    t.run_pending();
    let reply = t.take_replies().remove(0);
    assert_eq!(reply.status, ReplyStatus::Error);
    assert_eq!(decode_from::<String>(&reply.body), "port unavailable");

    // The platform can also say so later with status 2.
    t.host().script_port_async(TEST_PORT, ASK);
    ask(&t, h, 1, 2);
    t.run_pending();
    let id = t.host().port_calls()[1].port_call_id;
    t.runtime()
        .port_reply(&port_reply(id, PortStatus::Unavailable, &[]));
    t.run_pending();
    assert_eq!(
        decode_from::<String>(&t.take_replies()[0].body),
        "port unavailable"
    );
}

#[test]
fn malformed_and_unknown_port_replies_are_logged_and_ignored() {
    let t = TestRuntime::new();
    t.runtime().port_reply(&[1, 2]);
    t.runtime().port_reply(&port_reply_ok(12345, &[]));
    let mut bad_status = port_reply_ok(1, &[]);
    bad_status[4] = 77;
    t.runtime().port_reply(&bad_status);
    let logs = t.host().take_logs();
    assert_eq!(logs.iter().filter(|l| l.level == 3).count(), 3, "{logs:?}");
}

#[test]
fn a_reply_delivered_twice_is_only_applied_once() {
    let t = TestRuntime::new();
    t.host().script_port_async(TEST_PORT, ASK);
    let h = new_counter(&t, 0, "");
    ask(&t, h, 1, 1);
    t.run_pending();
    let id = t.host().port_calls()[0].port_call_id;
    t.runtime().port_reply(&port_reply_ok(id, &enc(&1_i32)));
    t.runtime().port_reply(&port_reply_ok(id, &enc(&2_i32)));
    t.run_pending();
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(decode_body::<i32>(&replies[0]), 1);
}

// ----- sync port calls --------------------------------------------------------------------

#[test]
fn sync_port_calls_need_a_synchronous_answer() {
    let t = TestRuntime::new();
    let rt = t.runtime();
    t.host().script_port_ok(TEST_PORT, ASK, vec![4, 2]);
    assert_eq!(
        keel_runtime::port_call_sync(rt, TEST_PORT, ASK, &[1]),
        Ok(vec![4, 2])
    );

    // The host says "later": that is `Unavailable` for a sync call, and the call is abandoned.
    let other = keel_meta::ids::port_method_id("TestPort", "other");
    t.host().script_port_async(TEST_PORT, other);
    assert_eq!(
        rt.port_call_sync(TEST_PORT, other, &[]),
        Err(PortError::Unavailable)
    );
    let late = t.host().port_calls().last().unwrap().port_call_id;
    t.host().take_logs();
    rt.port_reply(&port_reply_ok(late, &[]));
    assert!(
        t.host()
            .take_logs()
            .iter()
            .any(|l| l.message.contains("abandoned"))
    );

    // An unscripted port is unavailable.
    assert_eq!(rt.port_call_sync(999, 1, &[]), Err(PortError::Unavailable));
    // A typed error status is a failure, not unavailability.
    t.host()
        .script_port(TEST_PORT, other, |c: &PortCallRecord| {
            PortCallOutcome::Sync(port_reply(c.port_call_id, PortStatus::Error, &[9]))
        });
    assert_eq!(
        rt.port_call_sync(TEST_PORT, other, &[]),
        Err(PortError::Failed(vec![9]))
    );
}

/// A host that answers from inside `port_call`, through `port_reply`, and then says `Async`:
/// how a wasm host with a synchronous JS function behaves (SPEC 7).
struct ReentrantReplier {
    rt: OnceLock<Weak<Runtime>>,
    body: Vec<u8>,
}

impl Host for ReentrantReplier {
    fn reply(&self, _: u32, _: &[u8]) {}
    fn change_set(&self, _: &[u8]) {}
    fn stream_item(&self, _: u32, _: &[u8]) {}
    fn port_call(&self, _: u32, _: u32, id: u32, _: &[u8]) -> PortCallOutcome {
        if let Some(rt) = self.rt.get().and_then(Weak::upgrade) {
            rt.port_reply(&port_reply_ok(id, &self.body));
        }
        PortCallOutcome::Async
    }
    fn log(&self, _: u8, _: &str, _: &str) {}
}

#[test]
fn a_reply_delivered_from_inside_the_host_call_counts_as_synchronous() {
    let host = Arc::new(ReentrantReplier {
        rt: OnceLock::new(),
        body: vec![7],
    });
    let rt = Runtime::new(
        RuntimeConfig {
            core_threads: 0,
            ..RuntimeConfig::default()
        },
        host.clone(),
    )
    .unwrap();
    host.rt.set(Arc::downgrade(&rt)).unwrap();
    assert_eq!(rt.port_call_sync(1, 2, &[]), Ok(vec![7]));
    // The async form resolves on its first poll.
    let future = rt.port_call(1, 2, vec![]);
    let out = poll_once(future);
    assert_eq!(out, Some(Ok(vec![7])));
    rt.shutdown();
}

fn poll_once<F: std::future::Future + Unpin>(mut f: F) -> Option<F::Output> {
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    match std::pin::Pin::new(&mut f).poll(&mut cx) {
        std::task::Poll::Ready(v) => Some(v),
        std::task::Poll::Pending => None,
    }
}

#[test]
fn dropping_a_port_future_abandons_the_call() {
    let t = TestRuntime::new();
    t.host().script_port_async(TEST_PORT, ASK);
    let future = t.runtime().port_call(TEST_PORT, ASK, vec![]);
    let id = future.port_call_id();
    assert_eq!(future.port_call_id(), t.host().port_calls()[0].port_call_id);
    drop(future);
    t.host().take_logs();
    t.runtime().port_reply(&port_reply_ok(id, &[]));
    assert!(
        t.host()
            .take_logs()
            .iter()
            .any(|l| l.message.contains("abandoned"))
    );
}

#[test]
fn shutdown_fails_pending_port_calls_with_cancelled() {
    let t = TestRuntime::new();
    t.host().script_port_async(TEST_PORT, ASK);
    let future = t.runtime().port_call(TEST_PORT, ASK, vec![]);
    t.runtime().shutdown();
    assert_eq!(t.run_until(future), Err(PortError::Cancelled));
}

// ----- Rust bindings ----------------------------------------------------------------------

/// The port trait the fixtures implement in Rust, like a `#[keel::port]` trait.
trait Clocky: Send + Sync {
    fn now(&self) -> u32;
}

struct FakeClock {
    now: AtomicU32,
}

impl Clocky for FakeClock {
    fn now(&self) -> u32 {
        self.now.load(Ordering::SeqCst)
    }
}

fn fake_clock(now: u32) -> Arc<dyn Clocky> {
    Arc::new(FakeClock {
        now: AtomicU32::new(now),
    })
}

const CLOCKY: u32 = keel_meta::ids::port_id("Clocky");
const CLOCKY_NOW: u32 = keel_meta::ids::port_method_id("Clocky", "now");
const CLOCKY_ADD: u32 = keel_meta::ids::port_method_id("Clocky", "add");
const CLOCKY_LATER: u32 = keel_meta::ids::port_method_id("Clocky", "later");
const CLOCKY_FAIL: u32 = keel_meta::ids::port_method_id("Clocky", "fail");

/// What `#[keel::port]` generates: decode the call, run it on the implementation, encode the
/// outcome as `status u8` + body.
fn clocky_dispatch(imp: &(dyn Any + Send + Sync), method_id: u32, args: &[u8]) -> PortDispatch {
    let Some(clock) = imp.downcast_ref::<Arc<dyn Clocky>>() else {
        return PortDispatch::Sync(vec![2]);
    };
    let reply = |status: u8, body: Vec<u8>| {
        let mut bytes = vec![status];
        bytes.extend(body);
        bytes
    };
    match method_id {
        CLOCKY_NOW => PortDispatch::Sync(reply(0, enc(&clock.now()))),
        CLOCKY_ADD => {
            let n: i32 = decode_from(args);
            PortDispatch::Sync(reply(0, enc(&(clock.now() as i32 + n))))
        }
        CLOCKY_LATER => {
            let clock = Arc::clone(clock);
            PortDispatch::Async(Box::pin(async move {
                keel_runtime::executor::yield_now().await;
                reply(0, enc(&(clock.now() + 1000)))
            }))
        }
        CLOCKY_FAIL => PortDispatch::Sync(reply(1, enc(&7_u16))),
        _ => PortDispatch::Sync(vec![2]),
    }
}

keel_meta::inventory::submit! {
    PortDispatcher { port_id: CLOCKY, dispatch: clocky_dispatch }
}

/// A Rust fake standing in for `TEST_PORT.ask` (the port the `Counter` fixture calls).
trait Asker: Send + Sync {
    fn triple(&self, x: i32) -> i32;
}

struct Tripler;

impl Asker for Tripler {
    fn triple(&self, x: i32) -> i32 {
        x * 3
    }
}

fn asker_dispatch(imp: &(dyn Any + Send + Sync), method_id: u32, args: &[u8]) -> PortDispatch {
    match imp.downcast_ref::<Arc<dyn Asker>>() {
        Some(asker) if method_id == ASK => {
            let mut bytes = vec![0];
            bytes.extend(enc(&asker.triple(decode_from::<i32>(args))));
            PortDispatch::Sync(bytes)
        }
        _ => PortDispatch::Sync(vec![2]),
    }
}

keel_meta::inventory::submit! {
    PortDispatcher { port_id: TEST_PORT, dispatch: asker_dispatch }
}

#[test]
fn rust_bindings_follow_the_arc_arc_convention() {
    let t = TestRuntime::new();
    let ctx = t.ctx();
    assert!(ctx.rust_port::<dyn Clocky>(CLOCKY).is_none());
    let clock = fake_clock(5);
    // What generated accessors and fakes do: an `Arc<Arc<dyn Trait>>` behind `dyn Any`.
    ctx.bind_port::<dyn Clocky>(CLOCKY, Arc::new(clock.clone()));
    let fetched = ctx.rust_port::<dyn Clocky>(CLOCKY).unwrap();
    assert!(Arc::ptr_eq(&fetched, &clock));
    assert_eq!(fetched.now(), 5);
    assert!(
        ctx.rust_port::<dyn Asker>(CLOCKY).is_none(),
        "another trait"
    );
    assert!(
        ctx.rust_port::<dyn Clocky>(CLOCKY + 1).is_none(),
        "another port"
    );
    assert!(t.runtime().unbind_port(CLOCKY));
    assert!(!t.runtime().unbind_port(CLOCKY));
    assert!(ctx.rust_port::<dyn Clocky>(CLOCKY).is_none());
}

#[test]
fn bind_dyn_port_does_the_wrapping() {
    let t = TestRuntime::new();
    let ctx = t.ctx();
    let clock = fake_clock(9);
    t.runtime()
        .bind_dyn_port::<dyn Clocky>(CLOCKY, clock.clone());
    let fetched = ctx.rust_port::<dyn Clocky>(CLOCKY).unwrap();
    assert!(Arc::ptr_eq(&fetched, &clock));
    // Rebinding replaces.
    ctx.bind_dyn_port::<dyn Clocky>(CLOCKY, fake_clock(10));
    assert_eq!(ctx.rust_port::<dyn Clocky>(CLOCKY).unwrap().now(), 10);
    // Foreign again: no Rust binding is visible.
    t.runtime().bind_foreign_port(CLOCKY);
    assert!(ctx.rust_port::<dyn Clocky>(CLOCKY).is_none());
}

#[test]
fn raw_calls_to_a_rust_bound_port_run_the_implementation_through_its_dispatcher() {
    let t = TestRuntime::new();
    let rt = t.runtime();
    t.host().script_port_ok(CLOCKY, CLOCKY_NOW, vec![99]); // must never be used
    rt.bind_dyn_port::<dyn Clocky>(CLOCKY, fake_clock(41));

    // Sync method, sync call.
    assert_eq!(rt.port_call_sync(CLOCKY, CLOCKY_NOW, &[]), Ok(enc(&41_u32)));
    assert_eq!(
        rt.port_call_sync(CLOCKY, CLOCKY_ADD, &enc(&1_i32)),
        Ok(enc(&42_i32))
    );
    // Async call of a sync method resolves at once; of an async one, when the task runs.
    assert_eq!(
        t.run_until(rt.port_call(CLOCKY, CLOCKY_NOW, vec![])),
        Ok(enc(&41_u32))
    );
    let later = rt.port_call(CLOCKY, CLOCKY_LATER, vec![]);
    assert_eq!(later.port_call_id(), 0, "no host round trip");
    assert_eq!(t.run_until(later), Ok(enc(&1041_u32)));
    // A sync call cannot wait for an async implementation.
    assert_eq!(
        rt.port_call_sync(CLOCKY, CLOCKY_LATER, &[]),
        Err(PortError::Unavailable)
    );
    // Typed errors and unknown methods.
    assert_eq!(
        rt.port_call_sync(CLOCKY, CLOCKY_FAIL, &[]),
        Err(PortError::Failed(enc(&7_u16)))
    );
    assert_eq!(
        rt.port_call_sync(CLOCKY, 12345, &[]),
        Err(PortError::Unavailable)
    );
    // The host was never asked.
    assert!(t.host().port_calls().is_empty());
}

#[test]
fn a_rust_bound_port_without_a_dispatcher_is_unavailable_and_never_reaches_the_host() {
    let t = TestRuntime::new();
    let no_dispatcher = keel_meta::ids::port_id("NoDispatcherForThis");
    t.host().script_port_ok(no_dispatcher, 1, vec![1]);
    t.runtime()
        .bind_port::<dyn Any>(no_dispatcher, Arc::new(fake_clock(0)));
    assert_eq!(
        t.runtime().port_call_sync(no_dispatcher, 1, &[]),
        Err(PortError::Unavailable)
    );
    let future = t.runtime().port_call(no_dispatcher, 1, vec![]);
    assert_eq!(t.run_until(future), Err(PortError::Unavailable));
    assert!(t.host().port_calls().is_empty());
    // Handing the port back to the platform restores host routing.
    t.runtime().bind_foreign_port(no_dispatcher);
    assert_eq!(
        t.runtime().port_call_sync(no_dispatcher, 1, &[]),
        Ok(vec![1])
    );
}

#[test]
fn a_dispatched_method_calls_a_rust_fake_instead_of_the_platform() {
    // The `Counter::ask_port` fixture calls `TEST_PORT.ask` through `Ctx::port_call`. With a
    // Rust fake bound, the platform is not involved at all.
    let t = TestRuntime::new();
    t.host().script_port_ok(TEST_PORT, ASK, enc(&-1_i32)); // the platform would answer -1
    let fake: Arc<dyn Asker> = Arc::new(Tripler);
    t.runtime().bind_dyn_port::<dyn Asker>(TEST_PORT, fake);
    let h = new_counter(&t, 0, "");
    ask(&t, h, 7, 3);
    t.run_pending();
    assert_eq!(decode_body::<i32>(&t.take_replies()[0]), 21);
    assert!(t.host().port_calls().is_empty());
}

// ----- events -----------------------------------------------------------------------------

const CONNECTIVITY: u32 = keel_meta::ids::port_id("Connectivity");
const CHANGED: u32 = keel_meta::ids::port_method_id("Connectivity", "changed");

#[test]
fn events_fan_out_to_subscribers_in_order_on_the_core() {
    let t = TestRuntime::new();
    let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let (s1, s2) = (seen.clone(), seen.clone());
    let sub1 = t.ctx().events().subscribe(
        CONNECTIVITY,
        CHANGED,
        Box::new(move |payload| {
            // Runs on the core loop: the runtime is current.
            assert!(keel_runtime::Ctx::try_current().is_some());
            s1.lock().push(("first", payload.to_vec()));
        }),
    );
    let sub2 = t.ctx().events().subscribe(
        CONNECTIVITY,
        CHANGED,
        Box::new(move |payload| s2.lock().push(("second", payload.to_vec()))),
    );
    t.runtime().event(CONNECTIVITY, CHANGED, &[1, 4]);
    assert_eq!(
        *seen.lock(),
        [("first", vec![1, 4]), ("second", vec![1, 4])]
    );

    // Other events and other ports do not reach them.
    t.runtime().event(CONNECTIVITY, 12345, &[9]);
    t.runtime().event(7, CHANGED, &[9]);
    assert_eq!(seen.lock().len(), 2);

    // Dropping a subscription unsubscribes it.
    drop(sub1);
    t.runtime().event(CONNECTIVITY, CHANGED, &[2]);
    assert_eq!(seen.lock().len(), 3);
    assert_eq!(seen.lock()[2], ("second", vec![2]));
    drop(sub2);
    t.runtime().event(CONNECTIVITY, CHANGED, &[3]);
    assert_eq!(seen.lock().len(), 3);
}

#[test]
fn a_panicking_subscriber_is_logged_and_the_others_still_run() {
    let t = TestRuntime::new();
    let ran = Arc::new(AtomicU32::new(0));
    let r = ran.clone();
    let _bad = t.ctx().events().subscribe(
        CONNECTIVITY,
        CHANGED,
        Box::new(|_| panic!("subscriber kaboom")),
    );
    let _good = t.ctx().events().subscribe(
        CONNECTIVITY,
        CHANGED,
        Box::new(move |_| {
            r.fetch_add(1, Ordering::SeqCst);
        }),
    );
    t.runtime().event(CONNECTIVITY, CHANGED, &[]);
    t.runtime().event(CONNECTIVITY, CHANGED, &[]);
    assert_eq!(ran.load(Ordering::SeqCst), 2);
    let logs = t.host().take_logs();
    assert_eq!(
        logs.iter()
            .filter(|l| l.level == 5 && l.message.contains("subscriber kaboom"))
            .count(),
        2,
        "{logs:?}"
    );
}

#[test]
fn subscribers_can_write_signals_and_spawn_tasks() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 0, "");
    t.runtime().observe(h.0, COUNT_SIGNAL, true);
    t.host().take_change_sets();
    let counter = t.runtime().object::<Counter>(h.0).unwrap();
    let ctx = t.ctx();
    let sub = t.ctx().events().subscribe(
        CONNECTIVITY,
        CHANGED,
        Box::new(move |payload| {
            counter.count.set(i32::from(payload[0]));
            let counter = counter.clone();
            ctx.spawn(async move { counter.count.update(|c| *c += 1000) });
        }),
    );
    t.runtime().event(CONNECTIVITY, CHANGED, &[3]);
    assert_eq!(
        t.host().take_decoded_change_sets().len(),
        1,
        "the write in the subscriber"
    );
    t.run_pending();
    let sets = t.host().take_decoded_change_sets();
    assert_eq!(sets[0].entries[0].value, enc(&1003_i32));
    drop(sub);
}

#[test]
fn events_from_inside_the_core_are_refused_not_deadlocked() {
    // A subscriber that calls `event` again is re-entering the core loop.
    let t = TestRuntime::new();
    let rt = t.runtime().clone();
    let _sub = t.ctx().events().subscribe(
        CONNECTIVITY,
        CHANGED,
        Box::new(move |_| rt.event(CONNECTIVITY, CHANGED, &[])),
    );
    t.runtime().event(CONNECTIVITY, CHANGED, &[]);
    let logs = t.host().take_logs();
    assert!(
        logs.iter().any(|l| l.message.contains("E_REENTRANT")),
        "{logs:?}"
    );
}

// ----- abandoned calls are capped (review finding L7) ------------------------------------------

/// A host that never answers the calls its core gave up on used to leave one id per call in the
/// table for good. The set is capped, with a warning, and stats show it.
#[test]
fn l7_abandoned_port_calls_are_capped_and_the_eviction_is_logged() {
    let t = TestRuntime::new();
    t.host().script_port_async(TEST_PORT, ASK);
    let rt = t.runtime().clone();
    t.host().take_logs();
    let cap = keel_runtime::MAX_ABANDONED;
    for _ in 0..cap + 50 {
        drop(rt.port_call(TEST_PORT, ASK, Vec::new())); // the future is dropped: abandoned
    }
    let stats: serde_json::Value = serde_json::from_str(&rt.stats_json()).unwrap();
    assert_eq!(stats["abandoned_port_calls"], cap as u64);
    assert_eq!(stats["pending_port_calls"], 0);
    let warnings: Vec<_> = t
        .host()
        .take_logs()
        .into_iter()
        .filter(|l| l.level == 3 && l.message.contains("abandoned calls"))
        .collect();
    // Rate-limited (re-review NF2): the first eviction warns, then once per 1024. 50
    // evictions therefore warn exactly once, and the message carries the running total.
    assert_eq!(warnings.len(), 1, "the eviction warning is rate-limited");
    assert!(
        warnings[0].message.contains("forgot the"),
        "{}",
        warnings[0].message
    );

    // A late reply to a call that is still remembered is discarded quietly and its id is freed;
    // one to a forgotten call is logged as unknown.
    let calls = t.host().take_port_calls();
    let (oldest, newest) = (calls[0].port_call_id, calls[calls.len() - 1].port_call_id);
    rt.port_reply(&keel_runtime::testing::port_reply_ok(newest, &[]));
    rt.port_reply(&keel_runtime::testing::port_reply_ok(oldest, &[]));
    let logs = t.host().take_logs();
    assert!(
        logs.iter().any(|l| l
            .message
            .contains(&format!("no port call {oldest} is pending"))),
        "{logs:?}"
    );
    assert!(
        !logs.iter().any(|l| l
            .message
            .contains(&format!("no port call {newest} is pending"))),
        "{logs:?}"
    );
    let stats: serde_json::Value = serde_json::from_str(&rt.stats_json()).unwrap();
    assert_eq!(stats["abandoned_port_calls"], cap as u64 - 1);
}
