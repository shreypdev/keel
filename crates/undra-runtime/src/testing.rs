//! An in-process runtime for tests: [`TestRuntime`] with a [`RecordingHost`].
//!
//! `TestRuntime::new()` builds a real [`Runtime`] with no threads at all:
//!
//! * no `undra-core` thread: the executor runs on the test thread, inside
//!   [`run_pending`](TestRuntime::run_pending), [`run_until`](TestRuntime::run_until) and
//!   [`advance`](TestRuntime::advance);
//! * a **manual clock**: `sleep` only completes when [`advance`](TestRuntime::advance) moves
//!   time past its deadline, so timing tests are exact and instant;
//! * `spawn_blocking` runs its closure on a **real pool thread**, like a native runtime, so a
//!   closure that writes signals (forbidden: it does not hold the core lock) fails in a test
//!   exactly as it does in a native debug build. [`run_pending`](TestRuntime::run_pending),
//!   [`run_until`](TestRuntime::run_until) and [`advance`](TestRuntime::advance) wait for the
//!   closures to finish and run the tasks they wake, so a test sees their results without
//!   waiting by hand.
//!
//! The [`RecordingHost`] records every reply, change-set, stream item, log line and port call
//! the runtime emits, and answers port calls from a scriptable table (unscripted ports are
//! unavailable), so a test can assert on exactly what crossed the boundary.
//!
//! ```
//! use undra_runtime::executor::Notify;
//! use undra_runtime::testing::TestRuntime;
//! use std::sync::Arc;
//!
//! let t = TestRuntime::new();
//! // A spawned task and a future that waits on it, driven on this thread.
//! let done = Arc::new(Notify::new());
//! let signal = done.clone();
//! t.ctx().spawn(async move { signal.notify_one() });
//! let answer = t.run_until(async move {
//!     done.notified().await;
//!     42
//! });
//! assert_eq!(answer, 42);
//! ```
//!
//! See [`TestRuntime::advance`] for driving time.
//!
//! # Writing signals from test code
//!
//! Change-sets are routed to the runtime that is current on the writing thread (see
//! `docs/runtime-internals.md`, section 12). Writes made inside a dispatched call, a task, or
//! [`Ctx::txn`] find the test runtime by themselves; a direct `signal.set(..)` in the test body
//! needs `let _scope = t.ctx().enter();` first, or its change-set has nowhere to go. The thread
//! that created the `TestRuntime` may write signals directly; any other thread (a
//! `std::thread::spawn` in a test) trips the write-context check in debug builds, the way a host
//! thread would in production. A test that has to prove what happens when such a write does
//! reach the runtime (release-build behaviour) wraps it in [`unchecked_writes`].

use core::future::Future;
use core::task::{Context, Poll, Waker};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Wake;
use std::time::Duration;

use undra_wire::payload::{
    Call, CallTarget, ChangeSet, PortReply, PortStatus, Reply, ReplyStatus, StreamFlag, StreamItem,
};
use undra_wire::{Reader, Writer};
use parking_lot::{Condvar, Mutex};

use crate::config::{MODE_INPROC, RuntimeConfig};
use crate::ctx::Ctx;
use crate::host::{Host, PortCallOutcome};
use crate::runtime::{BuildOptions, Runtime, UncheckedWrites};

/// How long [`TestRuntime::run_pending`] waits for one blocking closure to finish.
const BLOCKING_SETTLE_LIMIT: Duration = Duration::from_secs(10);

/// Declares the calling thread a test driver: it plays the core, so its direct signal writes are
/// allowed (see the [module documentation](self)). `TestRuntime::new` does this for the thread
/// that creates it; call it yourself from a harness that builds a real `Runtime` without a core
/// thread and drives dispatchers and futures directly on the test thread. The declaration lasts
/// for the life of the thread, which for a test is the test.
pub fn drive_from_this_thread() {
    crate::runtime::mark_test_driver_thread();
}

/// Runs `f` on the calling thread with the debug write-context check lifted, so `f` can write
/// signals the way a release-build embedder thread can.
///
/// For tests that prove the runtime's lock-level guarantees hold without the checker: a signal
/// write from a thread that does not hold the core lock panics in debug builds otherwise
/// (see the [module documentation](self)). It lifts the check only; to have the write delivered
/// to a particular runtime, enter its scope too (`let _scope = ctx.enter();`). Production code
/// never needs this; it should move the write onto the core (a task or a dispatched call).
pub fn unchecked_writes<R>(f: impl FnOnce() -> R) -> R {
    let _unchecked = UncheckedWrites::enter();
    f()
}

/// `rt.call_sync(payload)` through the allocating path: the reference the zero-allocation reply
/// slot (ADR-028) must match byte for byte.
///
/// `call_sync` writes the reply of a generated dispatcher straight into a thread-local buffer;
/// this function keeps that buffer busy, so the same call is answered the way it is for `call`,
/// for a layer or for a hand-written dispatcher: the dispatcher returns a `DispatchResult` and
/// the runtime builds the reply from it. A test that compares the two (`call_sync_reference`
/// against `call_sync`) proves the fast path changed nothing on the wire.
///
/// ```
/// use undra_runtime::testing::{TestRuntime, call_sync_reference};
///
/// let t = TestRuntime::new();
/// let bad = [1, 2, 3];
/// assert_eq!(call_sync_reference(t.runtime(), &bad), t.runtime().call_sync(&bad));
/// ```
pub fn call_sync_reference(rt: &Runtime, payload: &[u8]) -> Vec<u8> {
    let _busy = crate::sync_out::Occupied::new();
    rt.call_sync(payload)
}

/// A decoded `Reply` the host received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyRecord {
    /// The call being answered.
    pub call_id: u32,
    /// How it ended.
    pub status: ReplyStatus,
    /// The reply body.
    pub body: Vec<u8>,
}

/// A decoded `StreamItem` the host received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamRecord {
    /// The stream's call id.
    pub call_id: u32,
    /// Item, end or error.
    pub flag: StreamFlag,
    /// The item or error body.
    pub body: Vec<u8>,
}

/// A log line the host received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogRecord {
    /// Log port level (0 trace ... 5 fatal).
    pub level: u8,
    /// The emitting module.
    pub target: String,
    /// The text.
    pub message: String,
}

/// A port call the runtime made into the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortCallRecord {
    /// The port.
    pub port_id: u32,
    /// The port method.
    pub method_id: u32,
    /// The core's id for the call; answer it with [`port_reply_ok`] and friends.
    pub port_call_id: u32,
    /// The encoded parameters.
    pub args: Vec<u8>,
}

/// One thing the host received, in arrival order across all kinds
/// ([`RecordingHost::take_timeline`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostEvent {
    /// A `Reply` for this call id.
    Reply(u32),
    /// A `ChangeSet`.
    ChangeSet,
    /// A `StreamItem` for this call id.
    StreamItem(u32),
    /// A port call with this `port_call_id`.
    PortCall(u32),
}

/// Decides how the [`RecordingHost`] answers one port method.
pub type PortScript = Arc<dyn Fn(&PortCallRecord) -> PortCallOutcome + Send + Sync>;

/// Builds a `Call` payload.
pub fn call_payload(target: CallTarget, call_id: u32, args: &[u8]) -> Vec<u8> {
    let mut w = Writer::with_capacity(21 + args.len());
    Call {
        target,
        call_id,
        args,
    }
    .encode(&mut w);
    w.into_vec()
}

/// Decodes a `Reply` payload.
///
/// # Panics
///
/// Panics if the payload is malformed (this is a test helper).
pub fn decode_reply(payload: &[u8]) -> ReplyRecord {
    let mut r = Reader::new(payload);
    let reply = match Reply::decode(&mut r) {
        Ok(reply) => reply,
        Err(e) => panic!("malformed Reply payload: {e}"),
    };
    ReplyRecord {
        call_id: reply.call_id,
        status: reply.status,
        body: reply.body.to_vec(),
    }
}

/// Builds a `PortReply` payload.
pub fn port_reply(port_call_id: u32, status: PortStatus, body: &[u8]) -> Vec<u8> {
    let mut w = Writer::with_capacity(5 + body.len());
    PortReply {
        port_call_id,
        status,
        body,
    }
    .encode(&mut w);
    w.into_vec()
}

/// A successful `PortReply` payload with `body`.
pub fn port_reply_ok(port_call_id: u32, body: &[u8]) -> Vec<u8> {
    port_reply(port_call_id, PortStatus::Ok, body)
}

/// A synchronous, successful answer to `call`, for use in a [`PortScript`].
pub fn sync_ok(call: &PortCallRecord, body: &[u8]) -> PortCallOutcome {
    PortCallOutcome::Sync(port_reply_ok(call.port_call_id, body))
}

#[derive(Default)]
struct HostState {
    replies: Vec<ReplyRecord>,
    change_sets: Vec<Vec<u8>>,
    stream_items: Vec<StreamRecord>,
    logs: Vec<LogRecord>,
    port_calls: Vec<PortCallRecord>,
    timer_sets: Vec<(u32, u64)>,
    scripts: HashMap<(u32, u32), PortScript>,
    timeline: Vec<HostEvent>,
}

/// A [`Host`] that records everything and answers port calls from a script table. See the
/// [module documentation](self).
#[derive(Default)]
pub struct RecordingHost {
    state: Mutex<HostState>,
    arrived: Condvar,
    own_timers: AtomicBool,
    schedules: AtomicUsize,
}

impl RecordingHost {
    /// Creates a host that records and answers nothing.
    pub fn new() -> RecordingHost {
        RecordingHost::default()
    }

    /// Removes and returns the replies received so far.
    pub fn take_replies(&self) -> Vec<ReplyRecord> {
        std::mem::take(&mut self.state.lock().replies)
    }

    /// Number of replies waiting to be taken.
    pub fn reply_count(&self) -> usize {
        self.state.lock().replies.len()
    }

    /// Removes and returns the raw change-set payloads received so far.
    pub fn take_change_sets(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.state.lock().change_sets)
    }

    /// Like [`take_change_sets`](RecordingHost::take_change_sets), decoded.
    ///
    /// # Panics
    ///
    /// Panics if a payload does not decode (that would be a runtime bug).
    pub fn take_decoded_change_sets(&self) -> Vec<ChangeSet> {
        self.take_change_sets()
            .iter()
            .map(|bytes| match ChangeSet::decode(&mut Reader::new(bytes)) {
                Ok(cs) => cs,
                Err(e) => panic!("the runtime emitted a malformed change-set: {e}"),
            })
            .collect()
    }

    /// Number of change-sets waiting to be taken.
    pub fn change_set_count(&self) -> usize {
        self.state.lock().change_sets.len()
    }

    /// Removes and returns the stream items received so far.
    pub fn take_stream_items(&self) -> Vec<StreamRecord> {
        std::mem::take(&mut self.state.lock().stream_items)
    }

    /// Removes and returns the log lines received so far.
    pub fn take_logs(&self) -> Vec<LogRecord> {
        std::mem::take(&mut self.state.lock().logs)
    }

    /// The port calls made so far (not cleared).
    pub fn port_calls(&self) -> Vec<PortCallRecord> {
        self.state.lock().port_calls.clone()
    }

    /// Removes and returns the port calls made so far.
    pub fn take_port_calls(&self) -> Vec<PortCallRecord> {
        std::mem::take(&mut self.state.lock().port_calls)
    }

    /// Removes and returns the arrival order of everything the host received (replies,
    /// change-sets, stream items, port calls), for asserting on ordering.
    pub fn take_timeline(&self) -> Vec<HostEvent> {
        std::mem::take(&mut self.state.lock().timeline)
    }

    /// Removes and returns the `(timer_id, delay_ms)` pairs the runtime asked the host to set.
    pub fn take_timer_sets(&self) -> Vec<(u32, u64)> {
        std::mem::take(&mut self.state.lock().timer_sets)
    }

    /// Makes this host own timers (`Host::timer_set` returns `true`), like the wasm host.
    /// The test then completes sleeps with `Runtime::timer_fired`.
    pub fn set_own_timers(&self, own: bool) {
        self.own_timers.store(own, Ordering::SeqCst);
    }

    /// How many times the runtime asked for `poll` through `Host::schedule`.
    pub fn schedule_count(&self) -> usize {
        self.schedules.load(Ordering::SeqCst)
    }

    /// Scripts the answer to port method `(port_id, method_id)`.
    pub fn script_port(
        &self,
        port_id: u32,
        method_id: u32,
        script: impl Fn(&PortCallRecord) -> PortCallOutcome + Send + Sync + 'static,
    ) {
        self.state
            .lock()
            .scripts
            .insert((port_id, method_id), Arc::new(script));
    }

    /// Scripts a synchronous success with a fixed `body`.
    pub fn script_port_ok(&self, port_id: u32, method_id: u32, body: Vec<u8>) {
        self.script_port(port_id, method_id, move |call| sync_ok(call, &body));
    }

    /// Scripts an asynchronous answer: the test replies later with `Runtime::port_reply`.
    pub fn script_port_async(&self, port_id: u32, method_id: u32) {
        self.script_port(port_id, method_id, |_| PortCallOutcome::Async);
    }

    /// Waits (up to `timeout`) until at least `n` replies have been recorded.
    pub fn wait_for_replies(&self, n: usize, timeout: Duration) -> bool {
        self.wait_until(timeout, |s| s.replies.len() >= n)
    }

    /// Waits (up to `timeout`) until at least `n` stream items have been recorded.
    pub fn wait_for_stream_items(&self, n: usize, timeout: Duration) -> bool {
        self.wait_until(timeout, |s| s.stream_items.len() >= n)
    }

    /// Waits (up to `timeout`) until at least `n` change-sets have been recorded.
    pub fn wait_for_change_sets(&self, n: usize, timeout: Duration) -> bool {
        self.wait_until(timeout, |s| s.change_sets.len() >= n)
    }

    fn wait_until(&self, timeout: Duration, done: impl Fn(&HostState) -> bool) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let mut state = self.state.lock();
        while !done(&state) {
            if self.arrived.wait_until(&mut state, deadline).timed_out() {
                return done(&state);
            }
        }
        true
    }
}

impl Host for RecordingHost {
    fn reply(&self, call_id: u32, payload: &[u8]) {
        let record = decode_reply(payload);
        assert_eq!(
            record.call_id, call_id,
            "Reply payload disagrees with call_id"
        );
        let mut state = self.state.lock();
        state.timeline.push(HostEvent::Reply(call_id));
        state.replies.push(record);
        drop(state);
        self.arrived.notify_all();
    }

    fn change_set(&self, payload: &[u8]) {
        let mut state = self.state.lock();
        state.timeline.push(HostEvent::ChangeSet);
        state.change_sets.push(payload.to_vec());
        drop(state);
        self.arrived.notify_all();
    }

    fn stream_item(&self, call_id: u32, payload: &[u8]) {
        let mut r = Reader::new(payload);
        let item = match StreamItem::decode(&mut r) {
            Ok(item) => item,
            Err(e) => panic!("malformed StreamItem payload: {e}"),
        };
        assert_eq!(
            item.call_id, call_id,
            "StreamItem payload disagrees with call_id"
        );
        let mut state = self.state.lock();
        state.timeline.push(HostEvent::StreamItem(call_id));
        state.stream_items.push(StreamRecord {
            call_id,
            flag: item.flag,
            body: item.body.to_vec(),
        });
        drop(state);
        self.arrived.notify_all();
    }

    fn port_call(
        &self,
        port_id: u32,
        method_id: u32,
        port_call_id: u32,
        args: &[u8],
    ) -> PortCallOutcome {
        let record = PortCallRecord {
            port_id,
            method_id,
            port_call_id,
            args: args.to_vec(),
        };
        let script = {
            let mut state = self.state.lock();
            state.timeline.push(HostEvent::PortCall(port_call_id));
            state.port_calls.push(record.clone());
            state.scripts.get(&(port_id, method_id)).cloned()
        };
        match script {
            Some(script) => script(&record),
            None => PortCallOutcome::Unavailable,
        }
    }

    fn log(&self, level: u8, target: &str, message: &str) {
        self.state.lock().logs.push(LogRecord {
            level,
            target: target.to_owned(),
            message: message.to_owned(),
        });
    }

    fn schedule(&self) {
        self.schedules.fetch_add(1, Ordering::SeqCst);
    }

    fn timer_set(&self, timer_id: u32, delay_ms: u64) -> bool {
        if self.own_timers.load(Ordering::SeqCst) {
            self.state.lock().timer_sets.push((timer_id, delay_ms));
            true
        } else {
            false
        }
    }
}

struct WakeFlag(AtomicBool);

impl Wake for WakeFlag {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// An in-process runtime with a recording host, driven by the test. See the
/// [module documentation](self).
pub struct TestRuntime {
    rt: Arc<Runtime>,
    host: Arc<RecordingHost>,
}

impl Default for TestRuntime {
    fn default() -> Self {
        TestRuntime::new()
    }
}

impl TestRuntime {
    /// A test runtime with the default configuration (`platform "test"`, every log level
    /// recorded). Init hooks are not run; call [`run_init_hooks`](TestRuntime::run_init_hooks)
    /// after binding fakes.
    pub fn new() -> TestRuntime {
        TestRuntime::with_config(RuntimeConfig {
            platform: "test".to_owned(),
            mode: MODE_INPROC.to_owned(),
            core_threads: 0,
            blocking_threads: 0,
            log_level: 0,
        })
    }

    /// A test runtime with `config` (`mode` may be `"dev"`; the thread settings are ignored,
    /// a test runtime never starts threads).
    ///
    /// # Panics
    ///
    /// Panics if `config.mode` is invalid.
    pub fn with_config(config: RuntimeConfig) -> TestRuntime {
        // The creating thread is this test's driver: its direct signal writes are allowed.
        drive_from_this_thread();
        let host = Arc::new(RecordingHost::new());
        let rt = match Runtime::build(
            config,
            host.clone(),
            BuildOptions {
                manual: true,
                run_hooks: false,
                register_global: false,
            },
        ) {
            Ok(rt) => rt,
            Err(e) => panic!("TestRuntime: {e}"),
        };
        TestRuntime { rt, host }
    }

    /// The runtime under test.
    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.rt
    }

    /// Raises this runtime's generation counter to at least `floor`, so a test can reach the
    /// exhaustion path without issuing billions of handles. Test runtimes own a private
    /// counter, so nothing leaks into other tests.
    #[doc(hidden)]
    pub fn raise_generation_floor(&self, floor: u32) {
        self.rt.objects().raise_generation_floor(floor);
    }

    /// A [`Ctx`] for the runtime.
    pub fn ctx(&self) -> Ctx {
        self.rt.ctx()
    }

    /// The recording host.
    pub fn host(&self) -> &Arc<RecordingHost> {
        &self.host
    }

    /// Runs the registered `InitHook`s (query hydration, ...).
    pub fn run_init_hooks(&self) {
        self.rt.run_init_hooks();
    }

    /// `Runtime::call` with a payload built from its parts.
    pub fn call(&self, target: CallTarget, call_id: u32, args: &[u8]) -> u32 {
        self.rt.call(&call_payload(target, call_id, args))
    }

    /// `Runtime::call_sync` with a payload built from its parts; returns the decoded reply.
    pub fn call_sync(&self, target: CallTarget, call_id: u32, args: &[u8]) -> ReplyRecord {
        decode_reply(&self.rt.call_sync(&call_payload(target, call_id, args)))
    }

    /// Polls until no task is ready and no blocking closure is still running; returns the
    /// number of polls. Blocking closures (`Ctx::spawn_blocking`) run on a real pool thread, so
    /// this waits (up to ten seconds per closure) for them to finish and runs the tasks they
    /// wake: when it returns, a test sees the same state as if they had run inline.
    pub fn run_pending(&self) -> usize {
        self.settle()
    }

    /// `Runtime::run_pending`, then waits for blocking closures and repeats until neither
    /// has anything left.
    fn settle(&self) -> usize {
        let mut polled = 0;
        loop {
            polled += self.rt.run_pending();
            if self.rt.blocking_in_flight() == 0
                || !self.rt.wait_blocking_progress(BLOCKING_SETTLE_LIMIT)
            {
                return polled;
            }
        }
    }

    /// Removes and returns the replies recorded so far.
    pub fn take_replies(&self) -> Vec<ReplyRecord> {
        self.host.take_replies()
    }

    /// Removes and returns the raw change-sets recorded so far.
    pub fn take_change_sets(&self) -> Vec<Vec<u8>> {
        self.host.take_change_sets()
    }

    /// Drives `future` to completion on the test thread, running the executor whenever the
    /// future is waiting. The future is polled with this runtime current, so
    /// `Ctx::current()` works inside it.
    ///
    /// # Panics
    ///
    /// Panics if the future is pending and nothing can make progress (no ready task and no
    /// wake-up): a deadlock, or a sleep that needs [`advance`](TestRuntime::advance), or a
    /// port call that needs `port_reply`. Failing loudly beats hanging the test run.
    pub fn run_until<F: Future>(&self, future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let flag = Arc::new(WakeFlag(AtomicBool::new(false)));
        let waker = Waker::from(flag.clone());
        let _scope = self.ctx().enter();
        loop {
            flag.0.store(false, Ordering::SeqCst);
            let mut cx = Context::from_waker(&waker);
            if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                return value;
            }
            let polled = self.settle();
            if polled == 0 && !flag.0.load(Ordering::SeqCst) {
                panic!(
                    "TestRuntime::run_until: the future is pending and nothing can make progress \
                     ({} timer(s) pending: use advance(); {} port call(s) pending: use port_reply(); \
                     {} blocking closure(s) still running)",
                    self.rt.pending_timers(),
                    self.host.port_calls().len(),
                    self.rt.blocking_in_flight(),
                );
            }
        }
    }

    /// Moves the manual clock forward by `duration`. Tasks that are already ready run first,
    /// at the current time; then every sleep that comes due fires in deadline order, and the
    /// woken tasks run after each one, so a task that sleeps again inside the window is
    /// served within the same call. Returns how many sleeps fired.
    ///
    /// ```
    /// use undra_runtime::testing::TestRuntime;
    /// use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
    /// use std::time::Duration;
    ///
    /// let t = TestRuntime::new();
    /// let ticks = Arc::new(AtomicU32::new(0));
    /// let (ctx, seen) = (t.ctx(), ticks.clone());
    /// t.ctx().spawn(async move {
    ///     for _ in 0..3 {
    ///         ctx.sleep(Duration::from_secs(10)).await;
    ///         seen.fetch_add(1, Ordering::SeqCst);
    ///     }
    /// });
    /// t.run_pending();
    /// assert_eq!(ticks.load(Ordering::SeqCst), 0);
    /// t.advance(Duration::from_secs(25));
    /// assert_eq!(ticks.load(Ordering::SeqCst), 2);
    /// t.advance(Duration::from_secs(5));
    /// assert_eq!(ticks.load(Ordering::SeqCst), 3);
    /// ```
    pub fn advance(&self, duration: Duration) -> usize {
        let rt = &self.rt;
        // Tasks that are ready run at the current time, before any time passes.
        self.settle();
        let fired = rt.advance_clock(duration, || {
            self.settle();
        });
        self.settle();
        fired
    }
}

impl Drop for TestRuntime {
    fn drop(&mut self) {
        self.rt.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_until_resolves_a_future_that_needs_a_spawned_task() {
        let t = TestRuntime::new();
        let done = Arc::new(crate::executor::Notify::new());
        let signal = done.clone();
        t.ctx().spawn(async move { signal.notify_one() });
        let out = t.run_until(async move {
            done.notified().await;
            7
        });
        assert_eq!(out, 7);
    }

    #[test]
    #[should_panic(expected = "nothing can make progress")]
    fn run_until_panics_instead_of_hanging_on_a_stuck_future() {
        let t = TestRuntime::new();
        t.run_until(std::future::pending::<()>());
    }

    #[test]
    #[should_panic(expected = "1 timer(s) pending")]
    fn stuck_message_mentions_pending_timers() {
        let t = TestRuntime::new();
        let ctx = t.ctx();
        t.run_until(async move { ctx.sleep(Duration::from_secs(1)).await });
    }

    #[test]
    fn scripted_ports_answer_and_unscripted_ports_are_unavailable() {
        let t = TestRuntime::new();
        t.host().script_port_ok(1, 2, vec![9]);
        assert_eq!(t.runtime().port_call_sync(1, 2, &[]), Ok(vec![9]));
        assert_eq!(
            t.runtime().port_call_sync(1, 3, &[]),
            Err(crate::PortError::Unavailable)
        );
        assert_eq!(t.host().port_calls().len(), 2);
    }

    #[test]
    fn helpers_round_trip() {
        let payload = call_payload(CallTarget::Function { method_id: 5 }, 9, &[1, 2]);
        let call = Call::decode(&mut Reader::new(&payload)).unwrap();
        assert_eq!((call.call_id, call.args), (9, &[1, 2][..]));
        let mut w = Writer::new();
        Reply {
            call_id: 9,
            status: ReplyStatus::Cancelled,
            body: &[],
        }
        .encode(&mut w);
        assert_eq!(
            decode_reply(w.as_slice()),
            ReplyRecord {
                call_id: 9,
                status: ReplyStatus::Cancelled,
                body: Vec::new()
            }
        );
        let pr = port_reply_ok(4, &[1]);
        assert_eq!(pr, [4, 0, 0, 0, 0, 1]);
    }
}
