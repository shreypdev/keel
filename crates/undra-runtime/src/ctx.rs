//! [`Ctx`]: the handle user code uses to reach the runtime (SPEC 5.3).
//!
//! A `Ctx` is a cheap clone of an `Arc<Runtime>`. Inside a dispatched call, and inside any
//! task the executor polls, [`Ctx::current`] returns the runtime that is executing on this
//! thread; the blocking pool and [`Ctx::enter`] set it the same way.
//!
//! The typed convenience accessors (`ctx.http()`, `ctx.kv()`, `ctx.clock()`, ...) and the
//! query API (`ctx.query()`, `ctx.mutate()`) are **not** here: they belong to `undra-ports`
//! and `undra-query`, which add them as extension traits over the primitives below
//! ([`port_call`](Ctx::port_call), [`rust_port`](Ctx::rust_port),
//! [`runtime`](Ctx::runtime)`.extension()`).
//!
//! # `Ctx` and `WeakCtx` (ADR-034)
//!
//! **A `Ctx` lives for a call or a task step; anything that outlives the call keeps a
//! [`WeakCtx`].** A `Ctx` is a strong reference: a `Ctx` kept by something the runtime itself
//! owns (a spawned task that loops, an event subscriber, a store field) is a reference cycle that
//! keeps the runtime, its `undra-core` thread and its timers alive until
//! [`shutdown`](Runtime::shutdown). A [`WeakCtx`] does not keep the runtime alive: it is
//! [upgraded](WeakCtx::upgrade) around each step and says, with a typed [`Gone`], when the
//! runtime has been shut down or dropped. The periodic-task idiom is
//!
//! ```
//! use std::time::Duration;
//! use undra_runtime::{Ctx, WeakCtx};
//!
//! fn start_polling(ctx: &Ctx) {
//!     let weak: WeakCtx = ctx.downgrade();
//!     ctx.spawn(async move {
//!         // Ends with a typed outcome when the runtime goes, instead of pinning it.
//!         while weak.sleep(Duration::from_secs(30)).await.is_ok() {
//!             let Ok(ctx) = weak.upgrade() else { break };
//!             refresh(&ctx).await; // the strong Ctx lives for this step only
//!         }
//!     });
//! }
//! # async fn refresh(_: &Ctx) {}
//! ```

use core::any::Any;
use core::fmt;
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use parking_lot::Mutex;
use slab::Slab;

use crate::blocking::BlockingTask;
use crate::executor::TaskId;
use crate::ports::{Events, PortError, PortFuture};
use crate::runtime::{Reentrant, Runtime};
use crate::timer::Sleep;

thread_local! {
    static CURRENT: RefCell<Option<Arc<Runtime>>> = const { RefCell::new(None) };
}

/// The runtime executing on this thread, if any.
pub(crate) fn current_runtime() -> Option<Arc<Runtime>> {
    CURRENT.try_with(|c| c.borrow().clone()).ok().flatten()
}

/// Restores the previously current runtime when dropped. Returned by [`Ctx::enter`].
///
/// Not `Send`: it must be dropped on the thread that created it.
///
/// ```compile_fail
/// fn assert_send<T: Send>() {}
/// assert_send::<undra_runtime::CtxScope>();
/// ```
#[must_use = "the runtime is only current while the scope is alive"]
pub struct CtxScope {
    previous: Option<Arc<Runtime>>,
    _not_send: PhantomData<*const ()>,
}

impl CtxScope {
    pub(crate) fn enter(runtime: Arc<Runtime>) -> CtxScope {
        let previous = CURRENT.with(|c| c.borrow_mut().replace(runtime));
        CtxScope {
            previous,
            _not_send: PhantomData,
        }
    }
}

impl Drop for CtxScope {
    fn drop(&mut self) {
        let previous = self.previous.take();
        let _ = CURRENT.try_with(|c| *c.borrow_mut() = previous);
    }
}

/// A handle to the runtime for user code. Clone freely.
#[derive(Clone)]
pub struct Ctx(pub(crate) Arc<Runtime>);

impl Ctx {
    /// The runtime executing on this thread: valid inside any dispatched call, any task the
    /// executor polls, a blocking-pool closure and a [`Ctx::enter`] scope.
    ///
    /// # Panics
    ///
    /// Panics outside all of those. Use [`Ctx::try_current`] when that is possible.
    pub fn current() -> Ctx {
        match Ctx::try_current() {
            Some(ctx) => ctx,
            None => panic!(
                "undra_runtime::Ctx::current() called outside an Undra call, task or Ctx::enter() scope"
            ),
        }
    }

    /// Like [`Ctx::current`], but `None` when no runtime is executing on this thread.
    pub fn try_current() -> Option<Ctx> {
        current_runtime().map(Ctx)
    }

    /// Makes this runtime the current one on this thread until the returned scope is dropped:
    /// [`Ctx::current`] answers it. It does not make the thread the core: signal writes still need
    /// the owning runtime's core lock ([`with_core`](Ctx::with_core), ADR-035), and their
    /// change-sets go to the store's owner whatever is current.
    pub fn enter(&self) -> CtxScope {
        CtxScope::enter(self.0.clone())
    }

    /// The runtime itself.
    pub fn runtime(&self) -> &Runtime {
        &self.0
    }

    /// Batches every signal write made inside `f` into one transaction (one change-set per
    /// store). Nested calls join the outer transaction. Delegates to `undra_signals::txn` with
    /// this runtime current; each change-set goes to its store's owner. On a thread that is not
    /// the core already, use [`with_core`](Ctx::with_core), which also takes the core lock the
    /// writes need (ADR-035).
    pub fn txn<R>(&self, f: impl FnOnce() -> R) -> R {
        let _scope = self.enter();
        undra_signals::txn(f)
    }

    /// Spawns a detached task. It runs on the core loop; its output is `()`. Returns an id for
    /// [`cancel_task`](Ctx::cancel_task). After the runtime was shut down this is a logged
    /// no-op: the future is dropped unpolled and the id names nothing.
    pub fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) -> TaskId {
        self.0.spawn(future)
    }

    /// Cancels a task spawned with [`spawn`](Ctx::spawn): its future is dropped and it is never
    /// polled again. A no-op for a task that has finished. The future is dropped **on the core**
    /// (with the core lock held, or queued for the core's next turn if the lock is busy), so
    /// user `Drop` code never runs concurrently with core code; this call never waits for the core.
    pub fn cancel_task(&self, id: TaskId) {
        self.0.cancel_task(id);
    }

    /// Runs `f` on the blocking pool (inline on wasm and in test runtimes) and resolves to
    /// its result. See the [`blocking`](crate::executor::BlockingTask) notes: `f` runs
    /// without the core lock.
    pub fn spawn_blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> BlockingTask<T> {
        self.0.spawn_blocking(f)
    }

    /// Completes after `duration` (through the host's timer if it owns one, else the
    /// internal timer). After the runtime was shut down it completes at once (and logs a warning).
    pub fn sleep(&self, duration: Duration) -> Sleep {
        self.0.sleep(duration)
    }

    /// Host-to-core event subscriptions (`Connectivity`, `Lifecycle`).
    pub fn events(&self) -> &Events {
        self.0.events()
    }

    /// Calls a platform-implemented async port method. The generated proxy encodes `args`. After
    /// the runtime was shut down nothing is sent: the future resolves to
    /// [`PortError::Cancelled`] (and a warning is logged).
    pub fn port_call(&self, port_id: u32, method_id: u32, args: Vec<u8>) -> PortFuture {
        self.0.port_call(port_id, method_id, args)
    }

    /// Calls a platform-implemented sync port method; see [`crate::port_call_sync`].
    pub fn port_call_sync(
        &self,
        port_id: u32,
        method_id: u32,
        args: &[u8],
    ) -> Result<Vec<u8>, PortError> {
        self.0.port_call_sync(port_id, method_id, args)
    }

    /// Binds a Rust implementation to a port id; see [`Runtime::bind_port`] for the
    /// convention (`imp` is an `Arc<Arc<dyn Trait>>`).
    pub fn bind_port<P: ?Sized + 'static>(&self, port_id: u32, imp: Arc<dyn Any + Send + Sync>) {
        self.0.bind_port::<P>(port_id, imp);
    }

    /// Binds an implementation as the trait object `P`; see [`Runtime::bind_dyn_port`].
    pub fn bind_dyn_port<P: ?Sized + Send + Sync + 'static>(&self, port_id: u32, imp: Arc<P>) {
        self.0.bind_dyn_port::<P>(port_id, imp);
    }

    /// The Rust binding of `port_id` as a `P` (`ctx.rust_port::<dyn Http>(port_id)`), if there is
    /// one: fakes and built-ins. What a generated port accessor tries before falling back to its
    /// proxy.
    pub fn rust_port<P: ?Sized + Send + Sync + 'static>(&self, port_id: u32) -> Option<Arc<P>> {
        self.0.rust_port::<P>(port_id)
    }

    /// A [`WeakCtx`] for this runtime: what anything that outlives the current call or task step
    /// keeps (a looping task, a subscriber, a field of a store), so that it does not keep the
    /// runtime alive (ADR-034).
    pub fn downgrade(&self) -> WeakCtx {
        WeakCtx {
            runtime: Arc::downgrade(&self.0),
            id: self.0.id(),
            lifeline: self.0.lifeline().clone(),
        }
    }

    /// Completes when the runtime starts shutting down, or at once if it already has, with
    /// [`Gone::ShutDown`]. Long-lived work selects on it instead of polling
    /// [`Runtime::is_shut_down`]. (A `Ctx` keeps its runtime alive, so it is never
    /// [`Gone::Dropped`] while one exists; see [`WeakCtx::closed`].)
    pub fn closed(&self) -> Closed {
        Closed::new(self.0.lifeline().clone())
    }

    /// Runs `f` on the calling thread **as the core**: takes the runtime's core lock, makes the
    /// runtime current, runs `f` inside one transaction and releases the lock (ADR-035).
    ///
    /// This is the sanctioned way for a host or embedder thread to write signals synchronously: a
    /// write to a store's signal from a thread that does not hold the owning runtime's core lock
    /// is refused (E0065). Everything else sends the value to the core instead (`ctx.spawn`, a
    /// call, a task that awaits [`spawn_blocking`](Ctx::spawn_blocking)'s result).
    ///
    /// ```
    /// use undra_runtime::testing::TestRuntime;
    /// use undra_signals::Signal;
    ///
    /// let t = TestRuntime::new();
    /// let ctx = t.ctx();
    /// let count = Signal::new(1);
    /// let seen = std::thread::spawn(move || ctx.with_core(|| { count.set(2); count.get() }))
    ///     .join()
    ///     .unwrap();
    /// assert_eq!(seen, Ok(2));
    /// ```
    ///
    /// # Errors
    ///
    /// [`Reentrant`] when the calling thread already holds this runtime's core lock or is inside
    /// one of its host callbacks (`E_REENTRANT`, SPEC 5.1): waiting for the lock there would
    /// deadlock. Inside a dispatched call or a task the thread *is* the core already: write
    /// directly.
    pub fn with_core<R>(&self, f: impl FnOnce() -> R) -> Result<R, Reentrant> {
        self.0.with_core(f)
    }
}

impl fmt::Debug for Ctx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("runtime", &self.0.id())
            .finish()
    }
}

// ----- WeakCtx ----------------------------------------------------------------------------------

/// Why a [`WeakCtx`] can no longer reach its runtime (ADR-034).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gone {
    /// [`Runtime::shutdown`] has begun. The memory may still be alive, but the runtime takes no
    /// more work and a holder never revives it.
    ShutDown,
    /// The last strong reference went: the runtime has been dropped.
    Dropped,
}

impl fmt::Display for Gone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Gone::ShutDown => "the runtime has been shut down",
            Gone::Dropped => "the runtime has been dropped",
        })
    }
}

impl std::error::Error for Gone {}

const OPEN: u8 = 0;
const SHUT_DOWN: u8 = 1;
const DROPPED: u8 = 2;

/// The runtime's "it is over" signal, shared by the runtime and every [`WeakCtx`], [`Closed`]
/// and [`WeakSleep`] made from it. It holds no reference to the runtime, so waiting on it does
/// not keep the runtime alive. Closed at the top of `shutdown` and in `Drop`, once: the first
/// reason wins.
#[derive(Default)]
pub(crate) struct Lifeline {
    state: AtomicU8,
    waiters: Mutex<Slab<Waker>>,
}

impl Lifeline {
    /// Records that the runtime is gone (first reason wins) and wakes every waiter.
    pub(crate) fn close(&self, why: Gone) {
        let code = match why {
            Gone::ShutDown => SHUT_DOWN,
            Gone::Dropped => DROPPED,
        };
        if self
            .state
            .compare_exchange(OPEN, code, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let waiters: Vec<Waker> = self.waiters.lock().drain().collect();
        for waker in waiters {
            // Wakers are arbitrary code: one that panics must not stop the rest, nor the
            // shutdown that is waking them.
            let _ = crate::guard::guarded(move || waker.wake());
        }
    }

    /// Why the runtime is gone, if it is.
    pub(crate) fn gone(&self) -> Option<Gone> {
        match self.state.load(Ordering::Acquire) {
            OPEN => None,
            SHUT_DOWN => Some(Gone::ShutDown),
            _ => Some(Gone::Dropped),
        }
    }
}

/// A handle to a runtime that does **not** keep it alive (ADR-034): what anything that outlives
/// a call or a task step keeps. `Clone + Send + Sync`; made with [`Ctx::downgrade`].
///
/// [`upgrade`](WeakCtx::upgrade) gives a [`Ctx`] for one step of work, or a typed [`Gone`] once
/// the runtime has started shutting down or has been dropped (a holder never revives a runtime
/// that is shutting down). [`sleep`](WeakCtx::sleep) and [`closed`](WeakCtx::closed) wait
/// without holding a strong reference. See the [module documentation](self) for the periodic
/// task idiom.
#[derive(Clone)]
pub struct WeakCtx {
    runtime: Weak<Runtime>,
    id: u64,
    lifeline: Arc<Lifeline>,
}

impl WeakCtx {
    /// A [`Ctx`] for the runtime, for one step of work.
    ///
    /// # Errors
    ///
    /// [`Gone::ShutDown`] once [`shutdown`](Runtime::shutdown) has begun, even while the memory is
    /// still alive; [`Gone::Dropped`] after the last strong reference went.
    pub fn upgrade(&self) -> Result<Ctx, Gone> {
        match self.runtime.upgrade() {
            Some(rt) if rt.is_shut_down() => Err(Gone::ShutDown),
            Some(rt) => Ok(Ctx(rt)),
            None => Err(Gone::Dropped),
        }
    }

    /// Whether the runtime is still running: neither shut down nor dropped. Cheap (two atomic
    /// loads), for loop conditions; [`upgrade`](WeakCtx::upgrade) is what gives access.
    pub fn is_alive(&self) -> bool {
        self.runtime.strong_count() > 0 && self.lifeline.gone().is_none()
    }

    /// Completes when the runtime starts shutting down or is dropped (or at once if it already
    /// has), with the reason. Holds no strong reference while pending.
    pub fn closed(&self) -> Closed {
        Closed::new(self.lifeline.clone())
    }

    /// Resolves `Ok(())` after `duration`, or `Err(Gone)` as soon as the runtime starts shutting
    /// down or is dropped, whichever comes first (it races the runtime's timer against
    /// [`closed`](WeakCtx::closed)). While pending it holds only the weak reference, so a
    /// sleeping loop never keeps its runtime alive; a runtime that is already gone resolves at
    /// once with the reason.
    ///
    /// ```
    /// use std::time::Duration;
    /// use undra_runtime::Gone;
    /// use undra_runtime::testing::TestRuntime;
    ///
    /// let t = TestRuntime::new();
    /// let weak = t.ctx().downgrade();
    /// let nap = weak.sleep(Duration::from_secs(1));
    /// t.runtime().shutdown();
    /// assert_eq!(t.run_until(nap), Err(Gone::ShutDown));
    /// ```
    pub fn sleep(&self, duration: Duration) -> WeakSleep {
        let closed = self.closed();
        let sleep = match self.upgrade() {
            Ok(ctx) => Some(ctx.0.sleep(duration)),
            Err(_) => None,
        };
        WeakSleep { sleep, closed }
    }

    /// The id of the runtime this handle refers to ([`Runtime::id`]).
    pub fn runtime_id(&self) -> u64 {
        self.id
    }
}

impl fmt::Debug for WeakCtx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WeakCtx")
            .field("runtime", &self.id)
            .field("alive", &self.is_alive())
            .finish()
    }
}

/// The future of [`Ctx::closed`] and [`WeakCtx::closed`]: completes, with the reason, when the
/// runtime starts shutting down or is dropped. Holds no strong reference to the runtime.
#[must_use = "futures do nothing unless awaited"]
pub struct Closed {
    lifeline: Arc<Lifeline>,
    /// This future's waker slot in the lifeline, while it is registered.
    key: Option<usize>,
}

impl Closed {
    fn new(lifeline: Arc<Lifeline>) -> Closed {
        Closed {
            lifeline,
            key: None,
        }
    }
}

impl Future for Closed {
    type Output = Gone;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Gone> {
        if let Some(gone) = self.lifeline.gone() {
            return Poll::Ready(gone);
        }
        let this = &mut *self;
        let mut waiters = this.lifeline.waiters.lock();
        // Checked again under the lock: `close` drains the waiters under it after setting the
        // state, so a registration that sees the state open is always woken.
        if let Some(gone) = this.lifeline.gone() {
            return Poll::Ready(gone);
        }
        match this.key.and_then(|key| waiters.get_mut(key)) {
            Some(waker) => {
                if !waker.will_wake(cx.waker()) {
                    waker.clone_from(cx.waker());
                }
            }
            None => this.key = Some(waiters.insert(cx.waker().clone())),
        }
        Poll::Pending
    }
}

impl Drop for Closed {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            let mut waiters = self.lifeline.waiters.lock();
            if waiters.contains(key) {
                waiters.remove(key);
            }
        }
    }
}

impl fmt::Debug for Closed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Closed")
            .field("gone", &self.lifeline.gone())
            .finish()
    }
}

/// The future of [`WeakCtx::sleep`]: `Ok(())` after the delay, `Err(Gone)` as soon as the runtime
/// starts shutting down or is dropped. Holds no strong reference to the runtime.
#[must_use = "futures do nothing unless awaited"]
pub struct WeakSleep {
    /// The runtime's sleep, or `None` when the runtime was already gone.
    sleep: Option<Sleep>,
    closed: Closed,
}

impl Future for WeakSleep {
    type Output = Result<(), Gone>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Gone>> {
        // The runtime going wins a tie: the loop that awaits this would only fail to upgrade.
        if let Poll::Ready(gone) = Pin::new(&mut self.closed).poll(cx) {
            return Poll::Ready(Err(gone));
        }
        match &mut self.sleep {
            Some(sleep) => Pin::new(sleep).poll(cx).map(Ok),
            // Unreachable in practice (a gone runtime closes the lifeline first); a sleep that
            // could never fire must not hang.
            None => Poll::Ready(Err(Gone::Dropped)),
        }
    }
}

impl fmt::Debug for WeakSleep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WeakSleep").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestRuntime;

    #[test]
    fn current_is_only_set_inside_a_scope_and_restores_on_drop() {
        assert!(Ctx::try_current().is_none());
        let a = TestRuntime::new();
        let b = TestRuntime::new();
        {
            let _sa = a.ctx().enter();
            assert_eq!(Ctx::current().runtime().id(), a.ctx().runtime().id());
            {
                let _sb = b.ctx().enter();
                assert_eq!(Ctx::current().runtime().id(), b.ctx().runtime().id());
            }
            assert_eq!(Ctx::current().runtime().id(), a.ctx().runtime().id());
        }
        assert!(Ctx::try_current().is_none());
    }

    #[test]
    #[should_panic(expected = "outside an Undra call")]
    fn current_panics_with_a_clear_message_outside_a_runtime() {
        let _ = Ctx::current();
    }

    #[test]
    fn ctx_is_send_and_sync_so_tasks_and_stores_can_hold_it() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Ctx>();
    }
}
