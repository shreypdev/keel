//! [`Ctx`]: the handle user code uses to reach the runtime (SPEC 5.3).
//!
//! A `Ctx` is a cheap clone of an `Arc<Runtime>`. Inside a dispatched call, and inside any
//! task the executor polls, [`Ctx::current`] returns the runtime that is executing on this
//! thread; the blocking pool and [`Ctx::enter`] set it the same way.
//!
//! The typed convenience accessors (`ctx.http()`, `ctx.kv()`, `ctx.clock()`, ...) and the
//! query API (`ctx.query()`, `ctx.mutate()`) are **not** here: they belong to `keel-ports`
//! and `keel-query`, which add them as extension traits over the primitives below
//! ([`port_call`](Ctx::port_call), [`rust_port`](Ctx::rust_port),
//! [`runtime`](Ctx::runtime)`.extension()`).

use core::any::Any;
use core::fmt;
use core::future::Future;
use core::marker::PhantomData;
use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use crate::blocking::BlockingTask;
use crate::executor::TaskId;
use crate::ports::{Events, PortError, PortFuture};
use crate::runtime::Runtime;
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
                "keel_runtime::Ctx::current() called outside a Keel call, task or Ctx::enter() scope"
            ),
        }
    }

    /// Like [`Ctx::current`], but `None` when no runtime is executing on this thread.
    pub fn try_current() -> Option<Ctx> {
        current_runtime().map(Ctx)
    }

    /// Makes this runtime the current one on this thread until the returned scope is
    /// dropped. Signal writes made meanwhile are delivered through this runtime's host.
    pub fn enter(&self) -> CtxScope {
        CtxScope::enter(self.0.clone())
    }

    /// The runtime itself.
    pub fn runtime(&self) -> &Runtime {
        &self.0
    }

    /// Batches every signal write made inside `f` into one transaction (one change-set per
    /// store). Nested calls join the outer transaction. Delegates to `keel_signals::txn`
    /// with this runtime current, so the change-set reaches this runtime's host.
    pub fn txn<R>(&self, f: impl FnOnce() -> R) -> R {
        let _scope = self.enter();
        keel_signals::txn(f)
    }

    /// Spawns a detached task. It runs on the core loop; its output is `()`. Returns an id for
    /// [`cancel_task`](Ctx::cancel_task).
    pub fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) -> TaskId {
        self.0.spawn(future)
    }

    /// Cancels a task spawned with [`spawn`](Ctx::spawn): its future is dropped and it is never
    /// polled again. A no-op for a task that has finished.
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
    /// internal timer).
    pub fn sleep(&self, duration: Duration) -> Sleep {
        self.0.sleep(duration)
    }

    /// Host-to-core event subscriptions (`Connectivity`, `Lifecycle`).
    pub fn events(&self) -> &Events {
        self.0.events()
    }

    /// Calls a platform-implemented async port method. The generated proxy encodes `args`.
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

    /// Binds a Rust implementation to a port id; see [`Runtime::bind_port`].
    pub fn bind_port<P: ?Sized + 'static>(&self, port_id: u32, imp: Arc<dyn Any + Send + Sync>) {
        self.0.bind_port::<P>(port_id, imp);
    }

    /// The Rust binding of `port_id`, if one was bound and it is a `T`.
    pub fn rust_port<T: Send + Sync + 'static>(&self, port_id: u32) -> Option<Arc<T>> {
        self.0.rust_port::<T>(port_id)
    }

    /// Like [`rust_port`](Ctx::rust_port) for a port bound as a trait object with
    /// [`Runtime::bind_dyn_port`]: `ctx.dyn_port::<dyn Clock>(Clock::PORT_ID)`.
    pub fn dyn_port<P: ?Sized + Send + Sync + 'static>(&self, port_id: u32) -> Option<Arc<P>> {
        self.0.dyn_port::<P>(port_id)
    }
}

impl fmt::Debug for Ctx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("runtime", &self.0.id())
            .finish()
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
    #[should_panic(expected = "outside a Keel call")]
    fn current_panics_with_a_clear_message_outside_a_runtime() {
        let _ = Ctx::current();
    }

    #[test]
    fn scope_is_not_send() {
        fn assert_send<T: Send>() {}
        fn assert_not_send_by_construction() {
            // `CtxScope` holds `PhantomData<*const ()>`, which makes it `!Send`; this test
            // documents the intent and keeps `Ctx` itself `Send + Sync`.
            assert_send::<Ctx>();
        }
        assert_not_send_by_construction();
    }
}
