//! Deterministic fakes of the standard ports, for tests and previews (SPEC 8).
//!
//! | Fake | Port(s) | Behaviour |
//! |---|---|---|
//! | [`FakeHttp`] | [`Http`] | scripted replies chosen by [`Matcher`], every request recorded |
//! | [`MemKv`] | [`Kv`] | in-memory ordered map, operations recorded, failures injectable ([`MemStore::fail`]) |
//! | [`FailingKv`] | [`Kv`] | every operation fails with one [`StorageError`](crate::StorageError) |
//! | [`MemSecureStore`] | [`SecureStore`] | same, under its own port id |
//! | [`MemFs`] | [`Fs`] | in-memory tree with the platform adapters' error semantics |
//! | [`FakeClock`] | [`Clock`] + [`Timer`] | settable time; `advance` fires due timers |
//! | [`SeededRng`] | [`Rng`] | xorshift64\*, same seed same bytes |
//! | [`CaptureLog`] | [`Log`] | keeps every record |
//! | [`ScriptedConnectivity`] | [`Connectivity`] | pushes scripted events into a runtime |
//! | [`ScriptedLifecycle`] | [`Lifecycle`] | pushes scripted events into a runtime |
//! | [`CaptureDiagnostics`] | [`Diagnostics`] | keeps every panic report |
//!
//! Every fake is `Send + Sync`, keeps its state behind a lock and never reads the system clock,
//! a random source or a thread (CLAUDE.md R12).
//!
//! [`install`] builds all of them and binds them into a
//! [`TestRuntime`]; [`Fakes`] is the bundle it returns.
//!
//! # Time
//!
//! [`FakeClock`] is the single source of time, for `Clock` and for `Timer`. On a
//! [`TestRuntime`], [`install`] makes the runtime's recording
//! host own the timers (like the web host does), so `ctx.sleep` asks for a timer and
//! [`Fakes::advance`] serves it from the fake clock: sleeps wake up at exactly their deadline, and
//! a task that reads `Clock::now_ms` when it wakes sees that deadline. Use [`Fakes::advance`], not
//! `TestRuntime::advance`, to move time on such a runtime.

mod clock;
mod diagnostics;
mod events;
mod fs;
mod http;
mod log;
mod rng;
mod store;

use core::time::Duration;
use std::sync::Arc;

use undra_runtime::testing::TestRuntime;
use undra_runtime::{Port, Runtime};

pub use clock::{FakeClock, MAX_TIMERS_PER_ADVANCE};
pub use diagnostics::CaptureDiagnostics;
pub use events::{ScriptedConnectivity, ScriptedLifecycle};
pub use fs::MemFs;
pub use http::{FakeHttp, Matcher};
pub use log::{CaptureLog, LogEntry};
pub use rng::SeededRng;
pub use store::{FailOn, FailingKv, MemKv, MemSecureStore, MemStore, StoreOp};

use crate::{
    BackgroundReport, Clock, Connectivity, Diagnostics, Fs, Http, Kv, Lifecycle, Log, Rng,
    SecureStore, Timer,
};

/// One of each fake, ready to be [installed](Fakes::install) into a runtime.
///
/// The fields are public `Arc`s: script and inspect them directly while the runtime holds its own
/// references.
#[derive(Clone, Debug)]
pub struct Fakes {
    /// The `Clock` and the `Timer`.
    pub clock: Arc<FakeClock>,
    /// The `Rng`.
    pub rng: Arc<SeededRng>,
    /// The `Log`.
    pub log: Arc<CaptureLog>,
    /// The `Diagnostics`: the panic reports of the runtime it is installed in.
    pub diagnostics: Arc<CaptureDiagnostics>,
    /// The `Http`.
    pub http: Arc<FakeHttp>,
    /// The `Kv`.
    pub kv: Arc<MemKv>,
    /// The `SecureStore`.
    pub secure_store: Arc<MemSecureStore>,
    /// The `Fs`.
    pub fs: Arc<MemFs>,
    /// The source of `Connectivity` events.
    pub connectivity: Arc<ScriptedConnectivity>,
    /// The source of `Lifecycle` events.
    pub lifecycle: Arc<ScriptedLifecycle>,
}

impl Fakes {
    /// Fresh fakes with default state: the [default time](FakeClock::DEFAULT_NOW_MS), the
    /// [default seed](SeededRng::DEFAULT_SEED), empty stores, no scripted replies, online on
    /// Wi-Fi and active.
    pub fn new() -> Fakes {
        Fakes::with_seed(SeededRng::DEFAULT_SEED)
    }

    /// Like [`new`](Fakes::new) with the random generator started from `seed`.
    pub fn with_seed(seed: u64) -> Fakes {
        Fakes {
            clock: Arc::new(FakeClock::new()),
            rng: Arc::new(SeededRng::new(seed)),
            log: Arc::new(CaptureLog::new()),
            diagnostics: Arc::new(CaptureDiagnostics::new()),
            http: Arc::new(FakeHttp::new()),
            kv: Arc::new(MemKv::new()),
            secure_store: Arc::new(MemSecureStore::new()),
            fs: Arc::new(MemFs::new()),
            connectivity: Arc::new(ScriptedConnectivity::new()),
            lifecycle: Arc::new(ScriptedLifecycle::new()),
        }
    }

    /// Binds every fake to `rt`:
    ///
    /// * the request/reply ports (`Clock`, `Rng`, `Log`, `Http`, `Kv`, `SecureStore`, `Fs`,
    ///   `Timer`) become Rust bindings, so both the typed accessors (`undra_ports::http(&ctx)`)
    ///   and raw port calls (the generated proxies) reach the fake;
    /// * the two event sources are attached to `rt`, so events they emit reach the subscribers
    ///   of `rt`'s [`Events`](undra_runtime::Events);
    /// * timers armed on the fake clock report `Runtime::timer_fired` when they fire, which is
    ///   what a platform timer does.
    ///
    /// Calling it again with another runtime binds the same fakes there too. To let the fake clock
    /// serve `ctx.sleep` on a [`TestRuntime`], use [`Fakes::install_test`] or [`install`].
    pub fn install(&self, rt: &Arc<Runtime>) {
        // With their dispatchers, so a raw port call (a generated proxy) reaches them too; the
        // standard ports' dispatchers are linked only where they are bound like this (ADR-052).
        rt.bind_dyn_port_with::<dyn Clock>(
            <dyn Clock as Port>::PORT_ID,
            self.clock.clone(),
            &crate::CLOCK_DISPATCHER,
        );
        rt.bind_dyn_port_with::<dyn Timer>(
            <dyn Timer as Port>::PORT_ID,
            self.clock.clone(),
            &crate::TIMER_DISPATCHER,
        );
        rt.bind_dyn_port_with::<dyn Rng>(
            <dyn Rng as Port>::PORT_ID,
            self.rng.clone(),
            &crate::RNG_DISPATCHER,
        );
        rt.bind_dyn_port_with::<dyn Log>(
            <dyn Log as Port>::PORT_ID,
            self.log.clone(),
            &crate::LOG_DISPATCHER,
        );
        rt.bind_dyn_port_with::<dyn Diagnostics>(
            <dyn Diagnostics as Port>::PORT_ID,
            self.diagnostics.clone(),
            &crate::DIAGNOSTICS_DISPATCHER,
        );
        rt.bind_dyn_port_with::<dyn Http>(
            <dyn Http as Port>::PORT_ID,
            self.http.clone(),
            &crate::HTTP_DISPATCHER,
        );
        rt.bind_dyn_port_with::<dyn Kv>(
            <dyn Kv as Port>::PORT_ID,
            self.kv.clone(),
            &crate::KV_DISPATCHER,
        );
        rt.bind_dyn_port_with::<dyn SecureStore>(
            <dyn SecureStore as Port>::PORT_ID,
            self.secure_store.clone(),
            &crate::SECURE_STORE_DISPATCHER,
        );
        rt.bind_dyn_port_with::<dyn Fs>(
            <dyn Fs as Port>::PORT_ID,
            self.fs.clone(),
            &crate::FS_DISPATCHER,
        );
        // Event ports flow host to core; binding the source only makes it discoverable with
        // `Ctx::rust_port`.
        rt.bind_dyn_port::<dyn Connectivity>(
            <dyn Connectivity as Port>::PORT_ID,
            self.connectivity.clone(),
        );
        rt.bind_dyn_port::<dyn Lifecycle>(<dyn Lifecycle as Port>::PORT_ID, self.lifecycle.clone());
        self.connectivity.attach(rt);
        self.lifecycle.attach(rt);
        // A weak reference: the runtime owns the clock, the clock must not own the runtime.
        let runtime = Arc::downgrade(rt);
        self.clock.on_timer_fired(move |timer_id| {
            if let Some(rt) = runtime.upgrade() {
                rt.timer_fired(timer_id);
            }
        });
    }

    /// [`install`](Fakes::install)s into `t`'s runtime and makes the fake clock the source of its
    /// timers: `ctx.sleep` on `t` is served by [`Fakes::advance`] from now on.
    pub fn install_test(&self, t: &TestRuntime) {
        self.install(t.runtime());
        t.host().set_own_timers(true);
    }

    /// Arms on the fake clock every timer the runtime has asked its host for since the last call
    /// (the sleeps its tasks started). [`advance`](Fakes::advance) does this itself; call it when
    /// you want [`FakeClock::pending_timers`] to show sleeps that started after a
    /// `t.run_pending()`. Only meaningful after [`install_test`](Fakes::install_test).
    pub fn sync_timers(&self, t: &TestRuntime) {
        for (timer_id, delay_ms) in t.host().take_timer_sets() {
            Timer::set(&*self.clock, timer_id, delay_ms);
        }
    }

    /// Runs the standard function `run_background(deadline)` on `t` and lets the window pass on
    /// the fake clock, the way an OS window does, until the run reports; returns the report.
    ///
    /// The run ends early when its tasks are done and at `deadline` less the half second it keeps
    /// for the host otherwise, so a task waiting for the network (see
    /// [`ScriptedConnectivity`]) shows as `finished: false` with its work still pending.
    ///
    /// ```
    /// use std::time::Duration;
    /// use undra_ports::fakes;
    /// use undra_runtime::testing::TestRuntime;
    ///
    /// let t = TestRuntime::new();
    /// let fakes = fakes::install(&t);
    /// // No background task is registered: nothing to do, so the run finishes at once.
    /// let report = fakes.run_background(&t, Duration::from_secs(30));
    /// assert!(report.finished && report.still_pending == 0);
    /// ```
    ///
    /// # Panics
    ///
    /// If the run has not reported a second after its deadline (a bug in the run).
    pub fn run_background(&self, t: &TestRuntime, deadline: Duration) -> BackgroundReport {
        let slot = Arc::new(parking_lot::Mutex::new(None));
        let (ctx, out) = (t.ctx(), slot.clone());
        t.ctx().spawn(async move {
            let ms = u64::try_from(deadline.as_millis()).unwrap_or(u64::MAX);
            *out.lock() = Some(crate::run_background(&ctx, ms).await);
        });
        let step = Duration::from_millis(50);
        let mut waited = Duration::ZERO;
        loop {
            self.advance(t, Duration::ZERO);
            if let Some(report) = slot.lock().take() {
                return report;
            }
            assert!(
                waited <= deadline + Duration::from_secs(1),
                "run_background did not report within its deadline"
            );
            self.advance(t, step);
            waited += step;
        }
    }

    /// Moves time forward by `duration`, one deadline at a time, and returns how many timers
    /// fired.
    ///
    /// At each deadline inside the window the fake clock reads exactly that instant, the due timer
    /// fires into the runtime (completing a `ctx.sleep`, or whatever else was armed through the
    /// `Timer` port), and the tasks it woke run before time moves on, so a task that sleeps again
    /// is served within the same call and a task that reads the clock sees its own deadline.
    /// Timers armed with a delay of zero fire at the end of the call.
    ///
    /// ```
    /// use std::sync::{Arc, Mutex};
    /// use std::time::Duration;
    /// use undra_ports::{Clock, fakes};
    /// use undra_runtime::testing::TestRuntime;
    ///
    /// let t = TestRuntime::new();
    /// let fakes = fakes::install(&t);
    /// let woke_at = Arc::new(Mutex::new(Vec::new()));
    /// let (ctx, clock, sink) = (t.ctx(), fakes.clock.clone(), woke_at.clone());
    /// t.ctx().spawn(async move {
    ///     for _ in 0..2 {
    ///         ctx.sleep(Duration::from_secs(10)).await;
    ///         sink.lock().unwrap().push(clock.now_ms());
    ///     }
    /// });
    ///
    /// let start = fakes.clock.now_ms();
    /// assert_eq!(fakes.advance(&t, Duration::from_secs(60)), 2);
    /// assert_eq!(*woke_at.lock().unwrap(), [start + 10_000, start + 20_000]);
    /// assert_eq!(fakes.clock.now_ms(), start + 60_000);
    /// ```
    ///
    /// # Panics
    ///
    /// After [`MAX_TIMERS_PER_ADVANCE`] timers with time still to move: a task that sleeps again
    /// without time passing would otherwise never end the call.
    pub fn advance(&self, t: &TestRuntime, duration: Duration) -> usize {
        let mut fired = 0;
        let mut remaining = duration;
        // Tasks that are ready run at the current time, before any time passes.
        t.run_pending();
        self.sync_timers(t);
        while !remaining.is_zero() {
            let step = self
                .clock
                .next_due_in()
                .map_or(remaining, |due| due.min(remaining));
            fired += self.clock.advance(step).len();
            // The runtime's own monotonic clock (what a background window is measured by) moves
            // with the fake one; its heap is empty here (the fake clock owns the timers).
            t.advance(step);
            assert!(
                fired <= MAX_TIMERS_PER_ADVANCE,
                "Fakes::advance fired {fired} timers: a task that sleeps again without time passing never ends (the cap is {MAX_TIMERS_PER_ADVANCE})"
            );
            t.run_pending();
            self.sync_timers(t);
            remaining -= step;
        }
        fired += self.clock.advance(Duration::ZERO).len();
        t.run_pending();
        self.sync_timers(t);
        fired
    }
}

impl Default for Fakes {
    fn default() -> Fakes {
        Fakes::new()
    }
}

/// Builds a [`Fakes`] and [installs](Fakes::install_test) it into `t`, with the fake clock as the
/// source of the runtime's timers.
///
/// ```
/// use undra_ports::{Clock, fakes};
/// use undra_runtime::testing::TestRuntime;
///
/// let t = TestRuntime::new();
/// let fakes = fakes::install(&t);
/// fakes.clock.set_now_ms(42);
/// assert_eq!(undra_ports::clock(&t.ctx()).now_ms(), 42);
/// ```
pub fn install(t: &TestRuntime) -> Fakes {
    let fakes = Fakes::new();
    fakes.install_test(t);
    fakes
}

/// A tiny executor for the unit tests of the fakes, whose futures are always ready.
#[cfg(test)]
pub(crate) mod testing {
    use core::future::Future;
    use core::task::{Context, Poll, Waker};

    /// Polls `future` once and returns its output; every fake answers immediately.
    pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("a fake port future was not ready on its first poll"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use undra_runtime::Host;

    #[test]
    fn new_fakes_start_in_the_documented_state() {
        let fakes = Fakes::new();
        assert_eq!(fakes.clock.now_ms(), FakeClock::DEFAULT_NOW_MS);
        assert_eq!(fakes.clock.monotonic_ns(), 0);
        assert_eq!(fakes.http.call_count(), 0);
        assert!(fakes.kv.is_empty() && fakes.secure_store.is_empty());
        assert!(fakes.fs.file_paths().is_empty());
        assert!(fakes.log.is_empty());
        assert_eq!(fakes.connectivity.current(), (true, crate::NetKind::Wifi));
        assert_eq!(fakes.lifecycle.current(), crate::AppState::Active);
        assert_eq!(
            Fakes::default().rng.fill(8),
            Fakes::with_seed(SeededRng::DEFAULT_SEED).rng.fill(8)
        );
        assert_ne!(
            Fakes::with_seed(1).rng.fill(8),
            Fakes::with_seed(2).rng.fill(8)
        );
    }

    #[test]
    fn install_test_makes_the_host_own_timers_and_plain_install_does_not() {
        let t = TestRuntime::new();
        assert!(
            !t.host().timer_set(1, 10),
            "a fresh test host has no timers"
        );
        let fakes = Fakes::new();
        fakes.install(t.runtime());
        assert!(
            !t.host().timer_set(1, 10),
            "install alone leaves timers to the runtime"
        );
        fakes.install_test(&t);
        assert!(
            t.host().timer_set(2, 20),
            "install_test hands timers to the fake clock"
        );
        assert_eq!(t.host().take_timer_sets(), [(2, 20)]);
    }

    #[test]
    fn sync_timers_moves_host_timer_requests_onto_the_fake_clock() {
        let t = TestRuntime::new();
        let fakes = install(&t);
        let ctx = t.ctx();
        t.ctx()
            .spawn(async move { ctx.sleep(Duration::from_millis(30)).await });
        t.run_pending();
        assert_eq!(fakes.clock.pending_timers(), 0, "not armed until synced");
        fakes.sync_timers(&t);
        assert_eq!(fakes.clock.next_due_in(), Some(Duration::from_millis(30)));
        fakes.sync_timers(&t);
        assert_eq!(
            fakes.clock.pending_timers(),
            1,
            "syncing twice arms nothing twice"
        );
    }

    #[test]
    fn advance_counts_fired_timers_and_lands_on_the_requested_time() {
        let t = TestRuntime::new();
        let fakes = install(&t);
        Timer::set(&*fakes.clock, 40, 10);
        Timer::set(&*fakes.clock, 41, 20);
        let start = fakes.clock.now_ms();
        assert_eq!(fakes.advance(&t, Duration::from_millis(15)), 1);
        assert_eq!(fakes.advance(&t, Duration::from_millis(15)), 1);
        assert_eq!(fakes.clock.now_ms(), start + 30);
        assert_eq!(fakes.clock.monotonic_ns(), 30_000_000);
    }

    #[test]
    #[should_panic(expected = "a task that sleeps again without time passing never ends")]
    fn a_task_that_sleeps_forever_stops_at_the_cap_instead_of_hanging_the_test() {
        let t = TestRuntime::new();
        let fakes = install(&t);
        let ctx = t.ctx();
        let inner = ctx.clone();
        ctx.spawn(async move {
            loop {
                inner.sleep(Duration::from_millis(1)).await;
            }
        });
        fakes.advance(&t, Duration::from_secs(1_000_000));
    }
}
