//! The write-context check: a hook the embedder uses to say which threads may mutate signals.
//!
//! The Undra runtime serialises all mutation on its core (SPEC 5.1); this crate cannot know what
//! "the core" is, so the runtime installs a checker. **Every build** asks it before a write that
//! has consequences for the host or for other nodes, and refuses the write when it says no
//! (ADR-035): a write from the wrong thread is a contract violation, and in a release build it
//! used to be applied and then silently not delivered, or delivered unordered.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::WriteError;

static CHECKER: OnceLock<fn(u64) -> bool> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Installs the process-wide write-context checker. Called by the runtime at init.
///
/// `f(owner)` answers "may the calling thread write a signal of the store owned by runtime
/// `owner` right now?". `owner` is the id the runtime recorded on the store's cell
/// ([`StoreCell::set_owner`](crate::StoreCell::set_owner)), or `0` for a signal that belongs to
/// no published store (unattached with dependents, or attached to a store not yet published), for
/// which the runtime accepts any thread that holds *a* core lock. The runtime's checker is an
/// allowlist (ADR-023, ADR-035): a thread that holds that runtime's core lock, a `TestRuntime`
/// driver thread, or one inside `testing::unchecked_writes`.
///
/// In **every build** each write to a signal that is attached to a store, or that has
/// dependents, asks `f` before anything changes; a refused write panics with the E0065 teaching
/// message (or, through [`Signal::try_set`](crate::Signal::try_set) and
/// [`Signal::try_update`](crate::Signal::try_update), returns [`WriteError::OffCore`]). A purely
/// local signal (unattached, nothing depends on it) is never checked.
///
/// The checker is process-wide and set once: the first `f` installed stays for the life of the
/// process (a second call only re-enables checking after [`clear_write_checker`]). While none is
/// enabled a write costs one relaxed atomic load.
///
/// # Example
///
/// ```
/// use undra_signals::{clear_write_checker, set_write_checker};
///
/// fn on_the_core(_owner: u64) -> bool {
///     true // the runtime checks its core lock here
/// }
/// set_write_checker(on_the_core);
/// # clear_write_checker();
/// ```
pub fn set_write_checker(f: fn(u64) -> bool) {
    // First installed wins: the runtime installs one checker per process, and a test binary
    // installs its own once.
    let _ = CHECKER.set(f);
    ENABLED.store(true, Ordering::Release);
}

/// Stops consulting the write-context checker. Used by tests; the runtime never needs to.
pub fn clear_write_checker() {
    ENABLED.store(false, Ordering::Release);
}

/// Whether the calling thread may write a signal owned by runtime `owner` (`0`: no published
/// store), according to the installed checker. Always `Ok` while no checker is enabled, and while
/// the thread is unwinding (a second panic there would abort the process).
pub(crate) fn check_write(owner: u64) -> Result<(), WriteError> {
    if !ENABLED.load(Ordering::Relaxed) {
        return Ok(());
    }
    let Some(check) = CHECKER.get() else {
        return Ok(());
    };
    if check(owner) || std::thread::panicking() {
        Ok(())
    } else {
        Err(WriteError::OffCore { owner })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_clear_round_trip() {
        // The checker is process-global (and the other unit tests of this binary write signals
        // in parallel), so this test installs one that allows everything and only exercises the
        // switch; refusals are tested in `tests/write_checker.rs`, which owns its process.
        fn yes(_: u64) -> bool {
            true
        }
        set_write_checker(yes);
        assert!(ENABLED.load(Ordering::Relaxed));
        assert_eq!(check_write(7), Ok(()));
        clear_write_checker();
        assert!(!ENABLED.load(Ordering::Relaxed));
        assert_eq!(check_write(7), Ok(()));
    }
}
