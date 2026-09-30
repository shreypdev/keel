//! Deterministic fakes of the standard ports, for tests and previews (SPEC 8).
//!
//! | Fake | Port(s) | Behaviour |
//! |---|---|---|
//! | [`FakeHttp`] | [`Http`](crate::Http) | scripted replies chosen by [`Matcher`], every request recorded |
//! | [`MemKv`] | [`Kv`](crate::Kv) | in-memory ordered map, operations recorded |
//! | [`MemSecureStore`] | [`SecureStore`](crate::SecureStore) | same, under its own port id |
//! | [`MemFs`] | [`Fs`](crate::Fs) | in-memory tree with the platform adapters' error semantics |
//! | [`FakeClock`] | [`Clock`](crate::Clock) + [`Timer`](crate::Timer) | settable time; `advance` fires due timers |
//! | [`SeededRng`] | [`Rng`](crate::Rng) | xorshift64\*, same seed same bytes |
//! | [`CaptureLog`] | [`Log`](crate::Log) | keeps every record |
//! | [`ScriptedConnectivity`] | [`Connectivity`](crate::Connectivity) | pushes scripted events into a runtime |
//! | [`ScriptedLifecycle`] | [`Lifecycle`](crate::Lifecycle) | pushes scripted events into a runtime |
//!
//! Every fake is `Send + Sync`, keeps its state behind a lock and never reads the system clock,
//! a random source or a thread (CLAUDE.md R12).
//!
//! [`install`] builds all of them and binds them into a
//! [`TestRuntime`](keel_runtime::testing::TestRuntime); [`Fakes`] is the bundle it returns.
//!
//! # Time
//!
//! A [`TestRuntime`](keel_runtime::testing::TestRuntime) has its own manual clock for
//! `ctx.sleep`, and [`FakeClock`] has one for `Clock` and `Timer`. [`Fakes::advance`] moves both
//! together, which is what a test almost always wants.

mod clock;
mod events;
mod fs;
mod http;
mod log;
mod rng;
mod store;

use core::time::Duration;
use std::sync::Arc;

use keel_runtime::testing::TestRuntime;
use keel_runtime::{Port, Runtime};

pub use clock::FakeClock;
pub use events::{ScriptedConnectivity, ScriptedLifecycle};
pub use fs::MemFs;
pub use http::{FakeHttp, Matcher};
pub use log::{CaptureLog, LogEntry};
pub use rng::SeededRng;
pub use store::{MemKv, MemSecureStore, MemStore, StoreOp};

use crate::{Clock, Connectivity, Fs, Http, Kv, Lifecycle, Log, Rng, SecureStore, Timer};

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
    ///   `Timer`) become Rust bindings, so both the typed accessors (`keel_ports::http(&ctx)`)
    ///   and raw port calls (the generated proxies) reach the fake;
    /// * the two event sources are attached to `rt`, so events they emit reach the subscribers
    ///   of `rt`'s [`Events`](keel_runtime::Events);
    /// * timers armed on the fake clock report `Runtime::timer_fired` when they fire, which is
    ///   what a platform timer does.
    ///
    /// Calling it again with another runtime binds the same fakes there too.
    pub fn install(&self, rt: &Arc<Runtime>) {
        rt.bind_dyn_port::<dyn Clock>(<dyn Clock as Port>::PORT_ID, self.clock.clone());
        rt.bind_dyn_port::<dyn Timer>(<dyn Timer as Port>::PORT_ID, self.clock.clone());
        rt.bind_dyn_port::<dyn Rng>(<dyn Rng as Port>::PORT_ID, self.rng.clone());
        rt.bind_dyn_port::<dyn Log>(<dyn Log as Port>::PORT_ID, self.log.clone());
        rt.bind_dyn_port::<dyn Http>(<dyn Http as Port>::PORT_ID, self.http.clone());
        rt.bind_dyn_port::<dyn Kv>(<dyn Kv as Port>::PORT_ID, self.kv.clone());
        rt.bind_dyn_port::<dyn SecureStore>(
            <dyn SecureStore as Port>::PORT_ID,
            self.secure_store.clone(),
        );
        rt.bind_dyn_port::<dyn Fs>(<dyn Fs as Port>::PORT_ID, self.fs.clone());
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

    /// Moves time forward by `duration` everywhere: the fake clock (firing its due timers into
    /// the runtime) and the test runtime's own clock (completing due `ctx.sleep`s and running the
    /// tasks they wake). Returns the ids of the timers the fake clock fired.
    pub fn advance(&self, t: &TestRuntime, duration: Duration) -> Vec<u32> {
        let fired = self.clock.advance(duration);
        t.advance(duration);
        fired
    }
}

impl Default for Fakes {
    fn default() -> Fakes {
        Fakes::new()
    }
}

/// Builds a [`Fakes`] and [installs](Fakes::install) it into `t`.
///
/// ```
/// use keel_ports::{Clock, fakes};
/// use keel_runtime::testing::TestRuntime;
///
/// let t = TestRuntime::new();
/// let fakes = fakes::install(&t);
/// fakes.clock.set_now_ms(42);
/// assert_eq!(keel_ports::clock(&t.ctx()).now_ms(), 42);
/// ```
pub fn install(t: &TestRuntime) -> Fakes {
    let fakes = Fakes::new();
    fakes.install(t.runtime());
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
