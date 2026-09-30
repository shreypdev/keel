//! Where committed change-sets go.

use std::cell::RefCell;
use std::sync::Arc;

use parking_lot::RwLock;

/// Receives every change-set the signals crate produces.
///
/// `change_set` is a complete `ChangeSet` payload (SPEC 3.5): `txn_id u64, count u32,
/// entries`. It is borrowed for the duration of the call.
///
/// `deliver` runs on the thread that committed the transaction, **after** all locks internal to
/// this crate have been released, so it may read and write signals. Writes made from inside
/// `deliver` do not commit re-entrantly: they are queued and committed, as a new transaction,
/// once the current commit finishes (see [`txn`](crate::txn)).
pub trait ChangeSink: Send + Sync {
    /// Handles one change-set payload.
    fn deliver(&self, change_set: &[u8]);
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
/// use keel_signals::testing::CaptureSink;
/// use keel_signals::{with_sink, Signal};
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
