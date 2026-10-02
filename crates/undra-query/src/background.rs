//! The query runtime's background tasks (ADR-046 decision 3): what `run_background` does with
//! the offline queue, the cache and persistence while the app is not on screen.
//!
//! Three tasks, registered on the runtime when the client starts ([`register`]):
//!
//! * **`undra-query.replay`** reads an unreadable queue again (ADR-049: an app launched before the
//!   device's first unlock), replays the queue if the client is online and waits until it is empty,
//!   offline or out of time. Offline it does not hold the OS's window: it ends at once with the work
//!   still pending, which the report says.
//! * **`undra-query.refetch`** fetches again the entries that are observed or persisted and stale,
//!   and waits for them.
//! * **`undra-query.flush`** writes at once what is waiting out its debounce (the cache entries and
//!   the queue) and waits for the writers.
//!
//! Each reports its progress as it goes ([`Deadline::note_replayed`](undra_runtime::background::Deadline)),
//! so a run the deadline cuts short still says what it got done. What a task started and the
//! deadline interrupted is not undone: the queue persists per item (ADR-037), a replay already in
//! flight belongs to the client and goes on, and a fetch that was cancelled is fetched again next
//! time.
//!
//! The tasks wait on a [`Notifier`] the client bumps whenever a replay step, a fetch, a write or a
//! hydration finishes, never by polling.

use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Poll, Waker};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use undra_runtime::background::{BackgroundFuture, BackgroundOutcome, Deadline};
use undra_runtime::{Ctx, WeakCtx};

use crate::shared::Shared;

/// Counts what finished and wakes whoever waits for the next thing to.
#[derive(Default)]
pub(crate) struct Notifier {
    epoch: AtomicU64,
    wakers: Mutex<Vec<Waker>>,
}

impl Notifier {
    /// Something finished: wake every waiter.
    pub(crate) fn bump(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let wakers = std::mem::take(&mut *self.wakers.lock());
        for waker in wakers {
            waker.wake();
        }
    }

    /// How many things have finished so far.
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// Resolves once something finishes after `seen` was read.
    fn changed(&self, seen: u64) -> impl Future<Output = ()> + '_ {
        poll_fn(move |cx| {
            if self.epoch() != seen {
                return Poll::Ready(());
            }
            {
                let mut wakers = self.wakers.lock();
                if !wakers.iter().any(|w| w.will_wake(cx.waker())) {
                    wakers.push(cx.waker().clone());
                }
            }
            // Registered first, checked again: a bump in between is not lost.
            if self.epoch() == seen {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
    }
}

/// Bumps the notifier when dropped: at the end of a task, also when it was cancelled or panicked.
pub(crate) struct BumpOnDrop(pub(crate) Arc<Shared>);

impl Drop for BumpOnDrop {
    fn drop(&mut self) {
        self.0.idle.bump();
    }
}

/// Why a wait ended.
pub(crate) enum Woke {
    /// Something finished.
    Changed,
    /// The window is over.
    Expired,
}

/// Waits for the next thing to finish (after `seen`) or for the window to end.
pub(crate) async fn wait(shared: &Shared, seen: u64, weak: &WeakCtx, deadline: &Deadline) -> Woke {
    let remaining = deadline.remaining();
    if remaining.is_zero() {
        return Woke::Expired;
    }
    let mut sleep = Box::pin(weak.sleep(remaining));
    let mut changed: Pin<Box<dyn Future<Output = ()> + Send + '_>> =
        Box::pin(shared.idle.changed(seen));
    poll_fn(move |cx| {
        if changed.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Woke::Changed);
        }
        if sleep.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Woke::Expired);
        }
        Poll::Pending
    })
    .await
}

/// Registers the three tasks on `ctx`'s runtime (idempotent: by name).
pub(crate) fn register(shared: &Arc<Shared>, ctx: &Ctx) {
    let runtime = ctx.runtime();
    let task = |shared: &Arc<Shared>,
                run: fn(Arc<Shared>, WeakCtx, Deadline) -> BackgroundFuture| {
        let shared = shared.clone();
        move |ctx: &Ctx, deadline: Deadline| run(shared.clone(), ctx.downgrade(), deadline)
    };
    let (probe_replay, probe_refetch, probe_flush) =
        (shared.clone(), shared.clone(), shared.clone());
    runtime.add_background_task(
        "undra-query.replay",
        move |_| probe_replay.background_queue_pending(),
        task(shared, |shared, weak, deadline| {
            Box::pin(async move { shared.background_replay(&weak, &deadline).await })
        }),
    );
    runtime.add_background_task(
        "undra-query.refetch",
        move |ctx| probe_refetch.background_refetch_pending(ctx),
        task(shared, |shared, weak, deadline| {
            Box::pin(async move { shared.background_refetch(&weak, &deadline).await })
        }),
    );
    runtime.add_background_task(
        "undra-query.flush",
        move |_| probe_flush.background_flush_pending(),
        task(shared, |shared, weak, deadline| {
            Box::pin(async move { shared.background_flush(&weak, &deadline).await })
        }),
    );
}

/// What a task that waits for work to end reports: done when `done`, incomplete otherwise.
pub(crate) fn outcome(done: bool) -> BackgroundOutcome {
    if done {
        BackgroundOutcome::Done
    } else {
        BackgroundOutcome::Incomplete
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::task::Context;
    use std::task::Wake;

    struct Flag(std::sync::atomic::AtomicBool);

    impl Wake for Flag {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn a_bump_after_the_epoch_was_read_wakes_the_waiter_and_one_before_it_is_not_lost() {
        let notifier = Notifier::default();
        let seen = notifier.epoch();
        let flag = Arc::new(Flag(std::sync::atomic::AtomicBool::new(false)));
        let waker = Waker::from(flag.clone());
        let mut cx = Context::from_waker(&waker);
        let mut changed = Box::pin(notifier.changed(seen));
        assert!(changed.as_mut().poll(&mut cx).is_pending());
        notifier.bump();
        assert!(flag.0.load(Ordering::SeqCst), "the waiter was woken");
        assert!(changed.as_mut().poll(&mut cx).is_ready());
        // A bump between reading the epoch and waiting is seen at once.
        let seen = notifier.epoch();
        notifier.bump();
        let mut late = Box::pin(notifier.changed(seen));
        assert!(late.as_mut().poll(&mut cx).is_ready());
    }
}
