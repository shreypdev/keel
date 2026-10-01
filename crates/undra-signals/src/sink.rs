//! Where committed change-sets go.

use std::cell::RefCell;
use std::sync::Arc;

use parking_lot::RwLock;

/// Receives every change-set the signals crate produces.
///
/// `change_set` is a complete `ChangeSet` payload (SPEC 3.5): `txn_id u64, count u32,
/// entries`. It is borrowed for the duration of the call.
///
/// `deliver` runs on the thread that committed the transaction. It may read and write signals:
/// writes made from inside `deliver` do not commit re-entrantly, they are queued and committed,
/// as a new transaction, once the current commit finishes (see [`txn`](crate::txn)).
///
/// # Ordering, and what a sink must not do
///
/// The change-sets of one store reach the sink one at a time, in the order they were built, with
/// strictly increasing transaction ids, whatever threads commit them. To guarantee that, the
/// store's **delivery lock is held while `deliver` runs** (it is taken when a commit claims the
/// store's dirty slots and released when `deliver` returns). Consequently a sink must not block
/// waiting for another thread that writes a signal of the same store: that thread would wait for
/// the delivery lock and neither would progress. Handing the payload to a queue, or writing
/// signals on the calling thread, is fine. Different stores do not exclude each other.
///
/// [`StoreCell::observe_and_deliver`](crate::StoreCell::observe_and_deliver) takes the same lock and
/// calls the caller's closure under it, so the same rule applies to that closure.
///
/// The runtime's sink only hands the payload to the host, which is why it can run under the lock.
pub trait ChangeSink: Send + Sync {
    /// Handles one change-set payload.
    fn deliver(&self, change_set: &[u8]);

    /// Handles one change-set of a store owned by runtime `owner` (its
    /// [`StoreCell::owner`](crate::StoreCell::owner); `0` for a store no runtime published). Every
    /// commit calls this, so a sink that serves several runtimes routes by the store's owner, not
    /// by the committing thread (ADR-035). The default forwards to [`deliver`](ChangeSink::deliver).
    fn deliver_from(&self, owner: u64, change_set: &[u8]) {
        let _ = owner;
        self.deliver(change_set);
    }

    /// Reports a write the checker refused (ADR-035), just before the writer panics with the
    /// E0065 message: `owner` is the runtime that owns the store (`0`: none), `message` the
    /// teaching text. The runtime's sink logs it at error level through that runtime, so the host
    /// hears of it even when the panicking thread is not one the runtime watches. The default does
    /// nothing. Not called by [`Signal::try_set`](crate::Signal::try_set) and friends, which return
    /// the refusal instead.
    fn off_core_write(&self, owner: u64, message: &str) {
        let _ = (owner, message);
    }

    /// Reports that a commit was cut off after `rounds` rounds because effects (or computeds
    /// and sinks) kept writing signals that triggered themselves.
    ///
    /// When the cap is hit the commit stops running effects, delivers the changes that are
    /// already dirty one last time, and releases the work it had queued (a slot or effect it
    /// still owned would otherwise be skipped by every other thread for good). This call is the
    /// error report for that: `undra-signals` has no log of its own, so the embedder's sink logs
    /// it (the runtime's sink logs at error level). The default does nothing. It runs after the
    /// store change-sets of the cut-off round were delivered, on the committing thread, with no
    /// store delivery lock held.
    fn round_cap_hit(&self, rounds: usize) {
        let _ = rounds;
    }
}

static GLOBAL: RwLock<Option<Arc<dyn ChangeSink>>> = RwLock::new(None);

thread_local! {
    static LOCAL: RefCell<Option<Arc<dyn ChangeSink>>> = const { RefCell::new(None) };
}

/// Installs the process-wide sink, replacing any previous one. Called by the runtime at init.
///
/// There is one sink per process (per instance on wasm). Until one is installed, commits still
/// update signals and run effects, but no change-sets are produced.
pub fn set_sink(sink: Arc<dyn ChangeSink>) {
    *GLOBAL.write() = Some(sink);
}

/// Removes the process-wide sink. Used by the runtime at shutdown.
pub fn clear_sink() {
    *GLOBAL.write() = None;
}

/// Runs `f` with `sink` receiving every change-set committed **on the calling thread**, in
/// preference to the process-wide sink. The previous override is restored when `f` returns or
/// unwinds.
///
/// This exists so that tests (and embedders that run several isolated cores in one process) can
/// observe commits without racing on the global. It does not affect commits made by other
/// threads.
///
/// # Example
///
/// ```
/// use undra_signals::testing::CaptureSink;
/// use undra_signals::{with_sink, Signal};
///
/// let capture = CaptureSink::new();
/// with_sink(capture.clone(), || {
///     Signal::new(1_u32).set(2); // unattached: nothing to deliver, but no interference either
/// });
/// assert!(capture.take().is_empty());
/// ```
pub fn with_sink<R>(sink: Arc<dyn ChangeSink>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Arc<dyn ChangeSink>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            let _ = LOCAL.try_with(|slot| *slot.borrow_mut() = previous);
        }
    }

    let previous = LOCAL
        .try_with(|slot| slot.borrow_mut().replace(sink))
        .unwrap_or(None);
    let _restore = Restore(previous);
    f()
}

/// The sink a commit on this thread should deliver to, if any.
pub(crate) fn current() -> Option<Arc<dyn ChangeSink>> {
    let local = LOCAL.try_with(|slot| slot.borrow().clone()).unwrap_or(None);
    local.or_else(|| GLOBAL.read().clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::CaptureSink;

    #[test]
    fn thread_override_wins_and_is_restored() {
        let outer = CaptureSink::new();
        let inner = CaptureSink::new();
        with_sink(outer.clone(), || {
            let cur = current().expect("sink installed");
            cur.deliver(&[1]);
            with_sink(inner.clone(), || {
                current().expect("inner sink").deliver(&[2]);
            });
            current().expect("outer restored").deliver(&[3]);
        });
        assert_eq!(outer.take(), vec![vec![1], vec![3]]);
        assert_eq!(inner.take(), vec![vec![2]]);
    }

    #[test]
    fn override_is_restored_after_a_panic() {
        let outer = CaptureSink::new();
        with_sink(outer.clone(), || {
            let result = std::panic::catch_unwind(|| {
                with_sink(CaptureSink::new(), || panic!("boom"));
            });
            assert!(result.is_err());
            current().expect("outer restored").deliver(&[7]);
        });
        assert_eq!(outer.take(), vec![vec![7]]);
    }

    #[test]
    fn override_is_thread_local() {
        let capture = CaptureSink::new();
        with_sink(capture.clone(), || {
            let seen_elsewhere = std::thread::spawn(|| {
                // Other threads do not see this thread's override. (They may see a global sink
                // installed by another test binary thread, which is why only the override is
                // compared.)
                LOCAL.with(|slot| slot.borrow().is_none())
            })
            .join()
            .expect("thread");
            assert!(seen_elsewhere);
        });
    }
}
