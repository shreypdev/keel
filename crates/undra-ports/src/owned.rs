//! Port calls that create something on the platform (a connection, a stream, a database, a
//! transaction), kept from leaking when their caller stops waiting.
//!
//! The platform finishes such a call whether or not the core still waits for the reply: a
//! cancelled port call is abandoned and the host is not told (SPEC 5.1). Awaited in place, a
//! `connect`, `open` or `begin` whose task is cancelled while it crosses therefore leaves an open
//! connection, stream, database or transaction that no handle in the core knows about, until the
//! runtime shuts down (a transaction left that way makes every later statement on its database
//! `Busy`). [`owned`] runs the call in a task of its own instead, and when the caller is gone by
//! the time the platform answers, it hands what was created to `orphan` (which closes it or rolls
//! it back). ADR-047 §7 and ADR-048 §5: dropping without closing closes, through a `WeakCtx`.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::Arc;

use parking_lot::Mutex;
use undra_runtime::WeakCtx;

/// A boxed future of nothing, what an `orphan` returns.
pub(crate) type Cleanup = Pin<Box<dyn Future<Output = ()> + Send>>;

type Call<T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send>>;
type Orphan<T> = Box<dyn FnOnce(T) -> Cleanup + Send>;

pub(crate) enum Slot<T, E> {
    /// The platform has not answered; the caller's waker, once it polled.
    Waiting(Option<Waker>),
    /// The answer, not taken yet.
    Done(Result<T, E>),
    /// The caller took the answer, or stopped waiting.
    Gone,
}

pub(crate) struct Shared<T, E> {
    slot: Mutex<Slot<T, E>>,
    orphan: Mutex<Option<Orphan<T>>>,
}

impl<T, E> Shared<T, E> {
    fn take_orphan(&self) -> Option<Orphan<T>> {
        self.orphan.lock().take()
    }
}

/// The future [`owned`] returns: the call's answer.
pub(crate) enum Owned<T, E> {
    /// No runtime to spawn on (no `WeakCtx`, or it shut down): the call is awaited in place.
    Inline(Call<T, E>),
    /// The call runs in its own task; this side waits for its answer.
    Spawned {
        shared: Arc<Shared<T, E>>,
        weak: WeakCtx,
    },
}

/// Runs `call` in a task of its own on `weak`'s runtime and answers what it answers. If this
/// future is dropped before it took a successful answer, `orphan` receives the value (when the
/// platform answers, or at once if it already had) and undoes it. Without a runtime to spawn on,
/// `call` is awaited in place, as before.
pub(crate) fn owned<T, E>(
    weak: Option<&WeakCtx>,
    call: impl Future<Output = Result<T, E>> + Send + 'static,
    orphan: impl FnOnce(T) -> Cleanup + Send + 'static,
) -> Owned<T, E>
where
    T: Send + 'static,
    E: Send + 'static,
{
    let Some((weak, ctx)) = weak.and_then(|weak| weak.upgrade().ok().map(|ctx| (weak, ctx))) else {
        return Owned::Inline(Box::pin(call));
    };
    let shared = Arc::new(Shared {
        slot: Mutex::new(Slot::Waiting(None)),
        orphan: Mutex::new(Some(Box::new(orphan) as Orphan<T>)),
    });
    let task = shared.clone();
    ctx.spawn(async move {
        let outcome = call.await;
        let (waker, orphaned) = {
            let mut slot = task.slot.lock();
            match core::mem::replace(&mut *slot, Slot::Gone) {
                Slot::Waiting(waker) => {
                    *slot = Slot::Done(outcome);
                    (waker, None)
                }
                Slot::Done(_) | Slot::Gone => (None, outcome.ok()),
            }
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        if let (Some(value), Some(orphan)) = (orphaned, task.take_orphan()) {
            orphan(value).await;
        }
    });
    // `ctx` is not kept: a call the platform never answers must not keep the runtime alive.
    Owned::Spawned {
        shared,
        weak: weak.clone(),
    }
}

impl<T, E> Future for Owned<T, E> {
    type Output = Result<T, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.get_mut() {
            Owned::Inline(call) => call.as_mut().poll(cx),
            Owned::Spawned { shared, .. } => {
                let mut slot = shared.slot.lock();
                match core::mem::replace(&mut *slot, Slot::Gone) {
                    Slot::Done(outcome) => Poll::Ready(outcome),
                    Slot::Waiting(_) => {
                        *slot = Slot::Waiting(Some(cx.waker().clone()));
                        Poll::Pending
                    }
                    // Polled after it completed: a contract breach of the caller; never panic.
                    Slot::Gone => Poll::Pending,
                }
            }
        }
    }
}

impl<T, E> Drop for Owned<T, E> {
    fn drop(&mut self) {
        let Owned::Spawned { shared, weak } = self else {
            return;
        };
        let taken = core::mem::replace(&mut *shared.slot.lock(), Slot::Gone);
        // Still waiting: the task sees `Gone` and hands the value to `orphan` when it arrives.
        // Answered but not taken (dropped between the wake and the next poll): undo it from here.
        if let Slot::Done(Ok(value)) = taken {
            if let (Ok(ctx), Some(orphan)) = (weak.upgrade(), shared.take_orphan()) {
                ctx.spawn(orphan(value));
            }
        }
    }
}
