//! `E_REENTRANT` covers every host callback, not just the core lock (ADR-023; review finding M2).
//!
//! SPEC 5.1 forbids a host to call back into the core from a callback. The runtime used to
//! detect that only for threads that hold the core lock. A callback delivered on another thread
//! (an off-core commit hands its change-set to the host while holding the store's delivery lock)
//! that called into the runtime waited for the core, while the core, inside a dispatch writing
//! that store, waited for the delivery lock: both hung. Now the runtime marks the thread for the
//! duration of every host callback and refuses re-entry with the documented error.

mod common;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use common::*;
use keel_runtime::testing::{ReplyRecord, call_payload, decode_reply, unchecked_writes};
use keel_runtime::{Host, PortCallOutcome, Runtime, RuntimeConfig};
use keel_wire::payload::ReplyStatus;
use parking_lot::Mutex;

/// A host that, from the callback it is armed for, calls `Runtime::call_sync` and `observe` and
/// records what came back.
struct ReenteringHost {
    rt: OnceLock<Weak<Runtime>>,
    /// Which thread's `change_set` callback to re-enter from (`None`: any thread).
    only_thread: Option<&'static str>,
    armed: AtomicBool,
    in_callback: AtomicBool,
    sleep_in_callback: Duration,
    results: Mutex<Vec<(&'static str, ReplyRecord)>>,
    logs: Mutex<Vec<String>>,
    change_sets: AtomicU32,
}

impl ReenteringHost {
    fn new(only_thread: Option<&'static str>, sleep_in_callback: Duration) -> Arc<ReenteringHost> {
        Arc::new(ReenteringHost {
            rt: OnceLock::new(),
            only_thread,
            armed: AtomicBool::new(false),
            in_callback: AtomicBool::new(false),
            sleep_in_callback,
            results: Mutex::new(Vec::new()),
            logs: Mutex::new(Vec::new()),
            change_sets: AtomicU32::new(0),
        })
    }

    fn runtime(&self) -> Arc<Runtime> {
        self.rt.get().and_then(Weak::upgrade).expect("runtime")
    }

    /// Re-enters the runtime from inside the callback named `from`.
    fn reenter(&self, from: &'static str) {
        if !self.armed.swap(false, Ordering::SeqCst) {
            return;
        }
        self.in_callback.store(true, Ordering::SeqCst);
        std::thread::sleep(self.sleep_in_callback);
        let rt = self.runtime();
        let reply =
            decode_reply(&rt.call_sync(&call_payload(function_target(SUM), 1, &sum_args())));
        self.results.lock().push((from, reply));
        // A void entry point is a logged no-op, not a hang either.
        rt.observe(1 << 32 | 1, u32::MAX, true);
        rt.release(1 << 32 | 1);
        rt.cancel(9);
    }

    fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }
}

fn sum_args() -> Vec<u8> {
    args(|w| {
        use keel_wire::Encode;
        1_i32.encode(w);
        2_i32.encode(w);
    })
}

impl Host for ReenteringHost {
    fn reply(&self, _: u32, _: &[u8]) {
        self.reenter("reply");
    }

    fn change_set(&self, _: &[u8]) {
        self.change_sets.fetch_add(1, Ordering::SeqCst);
        let on_wanted_thread = match self.only_thread {
            Some(name) => std::thread::current().name() == Some(name),
            None => true,
        };
        if on_wanted_thread {
            self.reenter("change_set");
        }
    }

    fn stream_item(&self, _: u32, _: &[u8]) {
        self.reenter("stream_item");
    }

    fn port_call(&self, _: u32, _: u32, _: u32, _: &[u8]) -> PortCallOutcome {
        self.reenter("port_call");
        PortCallOutcome::Unavailable
    }

    fn log(&self, _: u8, _: &str, message: &str) {
        if message.contains("E_REENTRANT") {
            self.logs.lock().push(message.to_owned());
        } else {
            self.reenter("log");
        }
    }

    fn timer_set(&self, _: u32, _: u64) -> bool {
        self.reenter("timer_set");
        false
    }

    fn schedule(&self) {
        self.reenter("schedule");
    }
}

fn runtime_with(host: Arc<ReenteringHost>, core_threads: u8) -> Arc<Runtime> {
    let rt = Runtime::new(
        RuntimeConfig {
            core_threads,
            log_level: 0,
            ..RuntimeConfig::default()
        },
        host.clone(),
    )
    .unwrap();
    let _ = host.rt.set(Arc::downgrade(&rt));
    rt
}

fn assert_refused(what: &str, reply: &ReplyRecord) {
    assert_eq!(reply.status, ReplyStatus::BadRequest, "{what}: {reply:?}");
    assert!(
        reason_of(reply).contains("E_REENTRANT"),
        "{what}: {}",
        reason_of(reply)
    );
}

/// The review's T5: thread `W` commits a write off the core and is delivering it to the host
/// (store delivery lock held) while the core, inside `call_sync(add)`, writes the same store and
/// waits for that lock. The host, inside `change_set` on `W`, calls back into the runtime.
#[test]
fn m2_an_off_core_callback_that_reenters_gets_e_reentrant_instead_of_deadlocking() {
    let host = ReenteringHost::new(Some("W"), Duration::from_millis(300));
    let rt = runtime_with(host.clone(), 1);
    let h = new_counter_rt(&rt, 0, "c");
    rt.observe(h.0, COUNT_SIGNAL, true);
    let counter = rt.object::<Counter>(h.0).unwrap();
    host.arm();

    let ctx = rt.ctx();
    let writer = std::thread::Builder::new()
        .name("W".into())
        .spawn(move || {
            let _scope = ctx.enter();
            // An off-core commit: holds the store's delivery lock while the host is called.
            unchecked_writes(|| counter.count.set(41));
        })
        .unwrap();
    while !host.in_callback.load(Ordering::SeqCst) {
        std::thread::yield_now();
    }
    // A host thread: takes the core lock, writes the same store and commits, so it queues for the
    // delivery lock that W holds for as long as its callback runs.
    let rt2 = rt.clone();
    let status = with_timeout(
        "call_sync(add) while an off-core callback re-enters",
        Duration::from_secs(10),
        move || call_sync_rt(&rt2, counter_target(h, ADD), 9, &enc(&1_i32)).status,
    );
    assert_eq!(
        status,
        ReplyStatus::Ok,
        "the core was not blocked by the re-entering callback"
    );
    with_timeout(
        "the writer thread finishes",
        Duration::from_secs(10),
        move || writer.join().unwrap(),
    );

    let results = host.results.lock();
    assert_eq!(results.len(), 1);
    assert_refused("call_sync from change_set", &results[0].1);
    let logs = host.logs.lock();
    assert!(
        logs.iter()
            .any(|l| l.contains("observe") && l.contains("E_REENTRANT")),
        "the void entry points log the refusal: {logs:?}"
    );
    assert!(logs.iter().any(|l| l.contains("release")), "{logs:?}");
    assert!(logs.iter().any(|l| l.contains("cancel")), "{logs:?}");
    rt.shutdown();
}

/// Every callback is covered, whichever thread makes it and whether or not it holds the core
/// lock: a test thread that calls `log`, `port_call_sync`, `sleep` and commits a write is not the
/// core, yet the host's re-entry from the resulting callback is refused.
#[test]
fn m2_reply_change_set_port_call_timer_set_and_log_callbacks_all_refuse_reentry() {
    let host = ReenteringHost::new(None, Duration::ZERO);
    let rt = runtime_with(host.clone(), 0);
    let h = new_counter_rt(&rt, 0, "c");
    let counter = rt.object::<Counter>(h.0).unwrap();
    rt.observe(h.0, COUNT_SIGNAL, true);

    // reply (on the caller's thread, under the core lock: the original detection).
    host.arm();
    assert_eq!(
        rt.call(&call_payload(function_target(SUM), 3, &sum_args())),
        0
    );
    // change_set from an off-core commit.
    host.arm();
    let ctx = rt.ctx();
    {
        let _scope = ctx.enter();
        unchecked_writes(|| counter.count.set(7));
    }
    // port_call, from a thread that does not hold the core lock.
    host.arm();
    let _ = rt.port_call_sync(TEST_PORT, ASK, &[]);
    // timer_set: a sleep started off the core.
    host.arm();
    let sleep = rt.sleep(Duration::from_secs(3600));
    drop(sleep);
    // log: any thread.
    host.arm();
    rt.log(4, "test", "a record");

    let results = host.results.lock();
    let froms: Vec<&str> = results.iter().map(|(from, _)| *from).collect();
    assert_eq!(
        froms,
        ["reply", "change_set", "port_call", "timer_set", "log"],
        "{froms:?}"
    );
    for (from, reply) in results.iter() {
        assert_refused(from, reply);
    }
    drop(results);
    rt.shutdown();
}
