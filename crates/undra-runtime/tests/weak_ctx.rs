//! ADR-034: anything that outlives a call holds a `WeakCtx`, and a runtime ends when its owner lets
//! go. These are the gap audit's lifecycle probes (LC-1, LC-2) as regression tests: before the
//! change every one of them left the dropped runtime alive, with its `undra-core`, timer and
//! blocking threads running.
//!
//! The thread counts are process-wide, so every test here takes [`SERIAL`] first.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use undra_meta::{DispatchCall, DispatchOutcome, FunctionMeta, Registration, TypeRefMeta, ids};
use undra_runtime::testing::{RecordingHost, TestRuntime, call_payload, live_threads};
use undra_runtime::{
    Ctx, DispatchBytes, DispatchResult, Gone, Runtime, RuntimeConfig, StoreObject, StoreRestorer,
    UndraObject, WeakCtx,
};
use undra_signals::{ALL_SIGNALS, Signal, StoreCell};
use undra_wire::payload::{CallTarget, ReplyStatus, StreamFlag};
use undra_wire::{Decode, Encode, Reader, Writer};

static SERIAL: Mutex<()> = Mutex::new(());

const PORT: u32 = ids::port_id("WeakCtxPort");
const ASK: u32 = ids::port_method_id("WeakCtxPort", "ask");
const EVENT_PORT: u32 = ids::port_id("WeakCtxEvents");
const PING: u32 = ids::port_method_id("WeakCtxEvents", "ping");

// ----- fixtures ---------------------------------------------------------------------------------

const WAIT_ON_PORT: u32 = ids::function_id("weak_wait_on_port");
const ENDLESS: u32 = ids::function_id("weak_endless");

/// An endless stream of numbers: the driver produces one and then waits for credit forever.
struct Endless(u32);

impl futures_core::Stream for Endless {
    type Item = DispatchBytes;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<DispatchBytes>> {
        self.0 += 1;
        Poll::Ready(Some(Ok(self.0.encode_to_vec())))
    }
}

fn functions(rt: &dyn Any, call: DispatchCall<'_>) -> DispatchOutcome {
    let Some(rt) = rt.downcast_ref::<Runtime>() else {
        return DispatchOutcome::new(DispatchResult::Unknown);
    };
    DispatchOutcome::new(match call.method_id {
        // An async call blocked on a port it will never hear back from. It keeps no `Ctx` across
        // the `.await` (the port future holds none), as ADR-034 asks of anything that waits.
        WAIT_ON_PORT => {
            let answer = rt.port_call(PORT, ASK, Vec::new());
            DispatchResult::Async(Box::pin(async move {
                let _ = answer.await;
                Ok(Vec::new())
            }))
        }
        ENDLESS => DispatchResult::Stream(Box::pin(Endless(0))),
        _ => DispatchResult::Unknown,
    })
}

static WAIT_META: FunctionMeta = FunctionMeta {
    name: "weak_wait_on_port",
    method_id: WAIT_ON_PORT,
    params: &[],
    returns: TypeRefMeta::Unit,
    is_async: true,
    takes_ctx: false,
    docs: "",
    dispatch: functions,
};
static ENDLESS_META: FunctionMeta = FunctionMeta {
    name: "weak_endless",
    method_id: ENDLESS,
    params: &[],
    returns: TypeRefMeta::Stream(&TypeRefMeta::U32),
    is_async: false,
    takes_ctx: false,
    docs: "",
    dispatch: functions,
};
undra_meta::inventory::submit! { Registration::Function(&WAIT_META) }
undra_meta::inventory::submit! { Registration::Function(&ENDLESS_META) }

/// A store that keeps its context the way ADR-034 recommends: a `WeakCtx`.
struct Keeper {
    cell: Arc<StoreCell>,
    value: Signal<i32>,
    ctx: WeakCtx,
}

impl Keeper {
    fn new(ctx: &Ctx, value: i32) -> Keeper {
        let cell = StoreCell::new(<Keeper as UndraObject>::TYPE_ID);
        let value = Signal::new(value);
        cell.attach(&value, 0).unwrap();
        Keeper {
            cell,
            value,
            ctx: ctx.downgrade(),
        }
    }
}

impl UndraObject for Keeper {
    const TYPE_ID: u32 = ids::type_id("WeakKeeper");
    const NAME: &'static str = "WeakKeeper";
}

impl StoreObject for Keeper {
    fn cell(&self) -> &Arc<StoreCell> {
        &self.cell
    }

    fn restore(ctx: Ctx, r: &mut Reader<'_>) -> Result<Self, undra_wire::WireError> {
        let signals = r.read_u32()?;
        let mut value = 0;
        for _ in 0..signals {
            let _id = r.read_u32()?;
            value = i32::decode(&mut Reader::new(r.read_bytes()?))?;
        }
        Ok(Keeper::new(&ctx, value))
    }
}

undra_meta::inventory::submit! {
    StoreRestorer {
        type_id: ids::type_id("WeakKeeper"),
        restore: |ctx, handle, r| {
            let restored = <Keeper as StoreObject>::restore(ctx, r)?;
            restored.cell().set_handle(handle);
            Ok(Arc::new(restored))
        },
        cell: |any| any.downcast_ref::<Keeper>().map(<Keeper as StoreObject>::cell),
    }
}

// ----- helpers ----------------------------------------------------------------------------------

/// A threaded runtime (a real `undra-core` thread, timer thread and blocking pool) that is not
/// the global one: what `undra dev` sessions and embedders that never call `shutdown` use.
fn threaded() -> (Arc<Runtime>, Arc<RecordingHost>) {
    let host = Arc::new(RecordingHost::new());
    let rt = Runtime::new(
        RuntimeConfig {
            platform: "test".to_owned(),
            mode: "inproc".to_owned(),
            core_threads: 1,
            blocking_threads: 2,
            log_level: 0,
        },
        host.clone(),
    )
    .expect("a runtime");
    (rt, host)
}

/// Drops the owner and asserts the runtime is freed within 100 ms and its threads have exited
/// (the brief's bar for every case).
fn drop_owner_and_expect_release(rt: Arc<Runtime>, threads_before: usize) {
    let weak: Weak<Runtime> = Arc::downgrade(&rt);
    drop(rt);
    let deadline = Instant::now() + Duration::from_millis(100);
    while weak.upgrade().is_some() || live_threads() > threads_before {
        assert!(
            Instant::now() < deadline,
            "the dropped runtime is still alive 100 ms after its owner let go \
             (alive: {}, runtime threads: {} > {threads_before})",
            weak.upgrade().is_some(),
            live_threads()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Waits (up to 5 s) for `done`.
fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

struct Unpark(std::thread::Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs `future` on the calling thread, outside every runtime (how a host or embedder thread
/// waits on a `WeakCtx`).
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
        std::thread::park_timeout(Duration::from_millis(50));
    }
}

fn call(rt: &Runtime, method_id: u32, call_id: u32) {
    let payload = call_payload(CallTarget::Function { method_id }, call_id, &[]);
    assert_eq!(rt.call(&payload), 0, "the call is accepted");
}

// ----- WeakCtx itself ---------------------------------------------------------------------------

#[test]
fn upgrade_is_shut_down_after_shutdown_and_dropped_after_drop() {
    let _serial = SERIAL.lock();
    let t = TestRuntime::new();
    let weak = t.ctx().downgrade();
    assert!(weak.is_alive());
    assert_eq!(weak.runtime_id(), t.runtime().id());
    let ctx = weak.upgrade().expect("a running runtime upgrades");
    assert_eq!(ctx.runtime().id(), t.runtime().id());
    drop(ctx);

    t.runtime().shutdown();
    assert!(!weak.is_alive());
    // The memory is still alive (the test holds the runtime), but a holder never revives it.
    assert_eq!(weak.upgrade().unwrap_err(), Gone::ShutDown);

    let (rt, _host) = threaded();
    let weak = rt.ctx().downgrade();
    drop(rt);
    assert!(!weak.is_alive());
    assert_eq!(weak.upgrade().unwrap_err(), Gone::Dropped);
    assert_eq!(Gone::Dropped.to_string(), "the runtime has been dropped");
    assert_eq!(Gone::ShutDown.to_string(), "the runtime has been shut down");
    drop(t);
}

#[test]
fn closed_completes_when_shutdown_starts_and_at_once_afterwards() {
    let _serial = SERIAL.lock();
    let (rt, _host) = threaded();
    let ctx = rt.ctx();
    let weak = ctx.downgrade();
    let strong_waiter = std::thread::spawn({
        let closed = ctx.closed();
        move || block_on(closed)
    });
    let weak_waiter = std::thread::spawn({
        let closed = weak.closed();
        move || block_on(closed)
    });
    std::thread::sleep(Duration::from_millis(20));
    assert!(!strong_waiter.is_finished() && !weak_waiter.is_finished());
    drop(ctx);
    rt.shutdown();
    assert_eq!(strong_waiter.join().unwrap(), Gone::ShutDown);
    assert_eq!(weak_waiter.join().unwrap(), Gone::ShutDown);
    // Afterwards: at once.
    assert_eq!(block_on(rt.ctx().closed()), Gone::ShutDown);
    assert_eq!(block_on(weak.closed()), Gone::ShutDown);
}

#[test]
fn weak_closed_says_dropped_when_the_owner_lets_go_without_shutdown() {
    let _serial = SERIAL.lock();
    let (rt, _host) = threaded();
    let weak = rt.ctx().downgrade();
    let waiter = std::thread::spawn({
        let closed = weak.closed();
        move || block_on(closed)
    });
    std::thread::sleep(Duration::from_millis(20));
    drop(rt);
    assert_eq!(waiter.join().unwrap(), Gone::Dropped);
}

#[test]
fn weak_sleep_completes_after_the_delay_and_ends_typed_when_shutdown_starts_mid_sleep() {
    let _serial = SERIAL.lock();
    let t = TestRuntime::new();
    let weak = t.ctx().downgrade();
    // Ok after the delay (the manual clock).
    let ok = Arc::new(Mutex::new(None));
    let (w, seen) = (weak.clone(), ok.clone());
    t.ctx().spawn(async move {
        *seen.lock() = Some(w.sleep(Duration::from_secs(5)).await);
    });
    t.run_pending();
    assert_eq!(*ok.lock(), None);
    t.advance(Duration::from_secs(5));
    assert_eq!(*ok.lock(), Some(Ok(())));

    // A threaded runtime shut down while a host thread sleeps on it: Err(ShutDown), promptly,
    // and the pending sleep does not keep the runtime alive.
    let threads_before = live_threads();
    let (rt, _host) = threaded();
    let weak = rt.ctx().downgrade();
    let sleeper = std::thread::spawn({
        let nap = weak.sleep(Duration::from_secs(3600));
        move || block_on(nap)
    });
    std::thread::sleep(Duration::from_millis(20));
    let started = Instant::now();
    rt.shutdown();
    assert_eq!(sleeper.join().unwrap(), Err(Gone::ShutDown));
    assert!(started.elapsed() < Duration::from_secs(1));
    drop_owner_and_expect_release(rt, threads_before);

    // On a runtime that is already gone it resolves at once, with the reason it ended (it was
    // shut down before it was dropped), while `upgrade` reports that the memory is gone.
    assert_eq!(
        block_on(weak.sleep(Duration::from_secs(3600))),
        Err(Gone::ShutDown)
    );
    assert_eq!(weak.upgrade().unwrap_err(), Gone::Dropped);
}

/// The periodic-task idiom of ADR-034 decision 9, running on a host thread: it ends with a typed
/// outcome when the owner drops the runtime, instead of spinning or pinning it.
#[test]
fn a_periodic_loop_on_a_host_thread_ends_with_dropped_and_does_not_pin_the_runtime() {
    let _serial = SERIAL.lock();
    let threads_before = live_threads();
    let (rt, _host) = threaded();
    let weak = rt.ctx().downgrade();
    let ticks = Arc::new(AtomicU32::new(0));
    let looper = std::thread::spawn({
        let ticks = ticks.clone();
        move || {
            block_on(async move {
                loop {
                    if let Err(gone) = weak.sleep(Duration::from_millis(2)).await {
                        return gone;
                    }
                    let Ok(ctx) = weak.upgrade() else {
                        return Gone::Dropped;
                    };
                    let _ = ctx.runtime().id();
                    ticks.fetch_add(1, Ordering::SeqCst);
                }
            })
        }
    });
    eventually("a few ticks", || ticks.load(Ordering::SeqCst) >= 3);
    drop_owner_and_expect_release(rt, threads_before);
    assert_eq!(looper.join().unwrap(), Gone::Dropped);
}

// ----- the runtime ends when its owner lets go ----------------------------------------------------

#[test]
fn lc1_an_open_endless_stream_does_not_pin_a_dropped_runtime_and_ends_with_one_cancelled_item() {
    let _serial = SERIAL.lock();
    let threads_before = live_threads();
    let (rt, host) = threaded();
    call(&rt, ENDLESS, 7);
    assert!(host.wait_for_replies(1, Duration::from_secs(5)));
    assert_eq!(host.take_replies()[0].status, ReplyStatus::StreamOpened);
    rt.stream_credit(7, 2);
    assert!(host.wait_for_stream_items(2, Duration::from_secs(5)));
    host.take_stream_items();

    drop_owner_and_expect_release(rt, threads_before);
    let items = host.take_stream_items();
    assert_eq!(items.len(), 1, "exactly one terminal item: {items:?}");
    assert_eq!(items[0].call_id, 7);
    assert_eq!(items[0].flag, StreamFlag::Error);
    let mut r = Reader::new(&items[0].body);
    assert_eq!(
        r.read_str().unwrap(),
        "cancelled: the runtime was dropped",
        "the host is told the stream did not end cleanly"
    );
}

#[test]
fn lc1_an_async_call_blocked_on_a_port_does_not_pin_a_dropped_runtime_and_is_answered_once() {
    let _serial = SERIAL.lock();
    let threads_before = live_threads();
    let (rt, host) = threaded();
    host.script_port_async(PORT, ASK);
    call(&rt, WAIT_ON_PORT, 9);
    eventually("the port call", || host.port_calls().len() == 1);

    drop_owner_and_expect_release(rt, threads_before);
    let replies = host.take_replies();
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert_eq!(
        (replies[0].call_id, replies[0].status),
        (9, ReplyStatus::Cancelled)
    );
}

#[test]
fn lc2_a_spawned_periodic_task_holding_a_weak_ctx_does_not_pin_the_runtime() {
    let _serial = SERIAL.lock();
    let threads_before = live_threads();
    let (rt, _host) = threaded();
    let ticks = Arc::new(AtomicU32::new(0));
    let weak = rt.ctx().downgrade();
    let counter = ticks.clone();
    rt.ctx().spawn(async move {
        while weak.sleep(Duration::from_millis(1)).await.is_ok() {
            let Ok(_ctx) = weak.upgrade() else { break };
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });
    eventually("the task to tick", || ticks.load(Ordering::SeqCst) >= 3);
    drop_owner_and_expect_release(rt, threads_before);
}

#[test]
fn lc2_a_subscriber_that_uses_the_ctx_it_is_given_does_not_pin_the_runtime() {
    let _serial = SERIAL.lock();
    let threads_before = live_threads();
    let (rt, _host) = threaded();
    let spawned = Arc::new(AtomicBool::new(false));
    let flag = spawned.clone();
    rt.events()
        .subscribe(
            EVENT_PORT,
            PING,
            Box::new(move |ctx: &Ctx, _: &[u8]| {
                // The subscriber reaches the runtime through its argument, not a capture.
                let flag = flag.clone();
                ctx.spawn(async move { flag.store(true, Ordering::SeqCst) });
            }),
        )
        .detach();
    rt.event(EVENT_PORT, PING, &[]);
    eventually("the subscriber's task", || spawned.load(Ordering::SeqCst));
    drop_owner_and_expect_release(rt, threads_before);
}

#[test]
fn lc2_a_store_with_a_weak_ctx_field_survives_snapshot_and_restore_and_does_not_pin_the_runtime() {
    let _serial = SERIAL.lock();
    let threads_before = live_threads();
    let (rt, host) = threaded();
    let handle = rt.insert_store(Arc::new(Keeper::new(&rt.ctx(), 41)));
    rt.observe(handle.0, ALL_SIGNALS, true);
    let snapshot = rt.snapshot();
    rt.restore(&snapshot).expect("restore");
    let restored = rt.object::<Keeper>(handle.0).expect("the same handle");
    assert_eq!(restored.value.get(), 41);
    assert_eq!(restored.ctx.upgrade().unwrap().runtime().id(), rt.id());
    drop(restored);
    assert!(host.change_set_count() >= 2, "observed, then restored");

    drop_owner_and_expect_release(rt, threads_before);
}

#[test]
fn an_idle_threaded_runtime_is_released_as_soon_as_its_owner_lets_go() {
    let _serial = SERIAL.lock();
    let threads_before = live_threads();
    let (rt, host) = threaded();
    // Start the timer thread and the pool, so there is something to join.
    let ctx = rt.ctx();
    let _ = block_on(async {
        ctx.sleep(Duration::from_millis(1)).await;
        ctx.spawn_blocking(|| 1).await
    });
    drop(ctx);
    assert!(live_threads() > threads_before);
    drop_owner_and_expect_release(rt, threads_before);
    assert!(host.take_replies().is_empty());
}

#[test]
fn stats_report_strong_refs_so_a_kept_ctx_is_visible() {
    let _serial = SERIAL.lock();
    let t = TestRuntime::new();
    let stats = |t: &TestRuntime| -> u64 {
        let json: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
        json["strong_refs"].as_u64().unwrap()
    };
    let base = stats(&t);
    let kept = t.ctx();
    assert_eq!(stats(&t), base + 1);
    let weak = kept.downgrade();
    drop(kept);
    assert_eq!(stats(&t), base, "a WeakCtx is not counted");
    drop(weak);
}

// ----- shutdown ordering ------------------------------------------------------------------------

/// Shutdown tells long-lived waiters first, then answers what is in flight, then stops the
/// threads; `Drop` after a `shutdown` answers nothing a second time.
#[test]
fn shutdown_closes_the_lifeline_before_it_answers_calls_and_drop_does_not_answer_twice() {
    let _serial = SERIAL.lock();
    let threads_before = live_threads();
    let (rt, host) = threaded();
    host.script_port_async(PORT, ASK);
    call(&rt, WAIT_ON_PORT, 3);
    eventually("the port call", || host.port_calls().len() == 1);
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let watcher = std::thread::spawn({
        let closed = rt.ctx().downgrade().closed();
        let order = order.clone();
        move || {
            let gone = block_on(closed);
            order.lock().push("closed");
            gone
        }
    });
    std::thread::sleep(Duration::from_millis(20));
    rt.shutdown();
    order.lock().push("shutdown returned");
    assert_eq!(watcher.join().unwrap(), Gone::ShutDown);
    let replies = host.take_replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].status, ReplyStatus::Cancelled);
    drop_owner_and_expect_release(rt, threads_before);
    assert!(
        host.take_replies().is_empty(),
        "Drop after shutdown answers nothing again"
    );
    assert_eq!(order.lock().len(), 2);
}

#[test]
fn a_runtime_dropped_from_its_own_core_thread_does_not_join_itself() {
    let _serial = SERIAL.lock();
    let threads_before = live_threads();
    let (rt, _host) = threaded();
    // The last strong reference is a task's: it lets go of it inside a poll, after the owner has
    // dropped the runtime, so `Drop` runs on the `undra-core` thread.
    let owner_gone = Arc::new(AtomicBool::new(false));
    let kept = Arc::new(Mutex::new(Some(rt.ctx())));
    let (gone, slot) = (owner_gone.clone(), kept.clone());
    rt.ctx().spawn(async move {
        while !gone.load(Ordering::SeqCst) {
            undra_runtime::executor::yield_now().await;
        }
        drop(slot.lock().take());
    });
    drop(kept);
    let weak: Weak<Runtime> = Arc::downgrade(&rt);
    let weak_ctx = rt.ctx().downgrade();
    drop(rt);
    assert!(weak.upgrade().is_some(), "the task's Ctx keeps it alive");
    owner_gone.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(5);
    while weak.upgrade().is_some() || live_threads() > threads_before {
        assert!(Instant::now() < deadline, "the runtime was not released");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(weak_ctx.upgrade().unwrap_err(), Gone::Dropped);
}

#[test]
fn writer_helpers_compile() {
    // Keeps `Writer` in use for the fixtures above on every target.
    let mut w = Writer::new();
    1_u8.encode(&mut w);
    assert_eq!(w.as_slice(), [1]);
}
