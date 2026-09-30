//! The write-context check: a hook the embedder uses to say which threads may mutate signals.
//!
//! The Keel runtime serialises all mutation on its core (SPEC 5.1); this crate cannot know what
//! "the core" is, so the runtime installs a checker. Debug builds call it on every write that
//! has consequences for the host or for other nodes, and assert on `false`. Release builds do
//! not evaluate it at all, so it costs nothing where it matters.

use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::RwLock;

static CHECKER: RwLock<Option<fn() -> bool>> = RwLock::new(None);
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Installs the process-wide write-context checker, replacing any previous one. Called by the
/// runtime at init.
///
/// `f` answers "may the calling thread write signals right now?": `true` on the core (a
/// dispatched call, a task poll), `false` on threads that must not mutate state: the blocking
/// pool, and, for the runtime's own checker, every thread that does not hold its core lock
/// (an allowlist since ADR-023). In **debug builds** every write to a signal that is
/// attached to a store, or that has dependents, asserts `f()` before it changes anything, so a
/// write from the wrong thread fails fast in tests instead of racing with the core: two threads
/// that write one store are not one transaction, and a write can be delivered with another
/// thread's transaction (see the crate docs, "Threading"). **Release builds never call `f`.**
///
/// While no checker is installed, a write costs one relaxed atomic load (debug builds) or
/// nothing (release builds).
///
/// # Example
///
/// ```
/// use keel_signals::{clear_write_checker, set_write_checker};
///
/// fn on_the_core() -> bool {
///     true // the runtime checks its core lock here
/// }
/// set_write_checker(on_the_core);
/// # clear_write_checker();
/// ```
pub fn set_write_checker(f: fn() -> bool) {
    *CHECKER.write() = Some(f);
    INSTALLED.store(true, Ordering::Relaxed);
}

/// Removes the write-context checker. Used by tests; the runtime never needs to.
pub fn clear_write_checker() {
    INSTALLED.store(false, Ordering::Relaxed);
    *CHECKER.write() = None;
}

/// Asserts that the calling thread may write signals, if a checker is installed.
///
/// Debug builds only. Skipped while the thread is already unwinding: a second panic there would
/// abort the process.
#[cfg(debug_assertions)]
pub(crate) fn assert_write_allowed() {
    if !INSTALLED.load(Ordering::Relaxed) || std::thread::panicking() {
        return;
    }
    let checker = *CHECKER.read();
    if let Some(check) = checker {
        assert!(
            check(),
            "keel-signals: a signal that is attached to a store (or has dependents) was written \
             from a thread that is not allowed to mutate state. Signal writes belong on the \
             core: send the result back to a task or a dispatched call instead of writing from \
             a blocking-pool or host thread (see docs/SPEC.md 5.1 and 16.1)."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_clear_round_trip() {
        // The checker is process-global, so this unit test only exercises the switch; the
        // behaviour is tested in `tests/write_checker.rs`, which owns its process.
        fn yes() -> bool {
            true
        }
        set_write_checker(yes);
        assert!(INSTALLED.load(Ordering::Relaxed));
        #[cfg(debug_assertions)]
        assert_write_allowed();
        clear_write_checker();
        assert!(!INSTALLED.load(Ordering::Relaxed));
    }
}
