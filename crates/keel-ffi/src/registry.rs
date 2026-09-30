//! The C ABI's port registrations (SPEC 6.3) and the rule that keeps a host's `user` pointer valid
//! for exactly as long as a callback can still use it (ADR-026).
//!
//! `keel_port_register` hands the core a `(callback, user)` pair. The host is allowed to free
//! `user` once the registration has been removed, so removal cannot return while a callback of
//! that registration is running on another thread, and no callback may start afterwards. Each
//! registration therefore counts its running invocations:
//!
//! * a port call finds the registration and [`Registry::enter`]s it, which counts one invocation
//!   and fails once the registration has been retired (it then looks again, and finds the
//!   replacement, if any);
//! * removal ([`Registry::install`] over an old one, [`Registry::remove`], [`Registry::retire_all`])
//!   takes the registration out of the map, marks it retired and **waits** until the count drops
//!   to zero before returning.
//!
//! The wait never happens under the map's lock, so a callback may still reach other ports while a
//! removal is waiting. A removal issued from inside a callback of the very registration it removes
//! would wait for itself; it is a contract violation (debug builds assert, release builds skip the
//! wait for the invocations of the calling thread).

use core::cell::RefCell;
use core::ffi::c_void;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock};

use crate::native::KeelPortCb;

/// An opaque host pointer handed back to its callbacks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct UserPtr(pub(crate) *mut c_void);

// SAFETY: the pointer is never dereferenced here, only passed back to the host's own callbacks.
// `keel.h` makes the host responsible for those callbacks being callable from any thread.
unsafe impl Send for UserPtr {}
// SAFETY: as above; sharing the value shares no data.
unsafe impl Sync for UserPtr {}

thread_local! {
    /// The serials of the registrations whose callbacks are running on this thread, innermost
    /// last. A removal consults it to tell "a callback of mine is running further up this
    /// stack" (which it can never wait for) from "a callback is running on another thread".
    static RUNNING: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

/// Hands out a distinct serial to every registration, so a replaced registration with the same
/// callback and `user` is still a different one.
static SERIALS: AtomicU64 = AtomicU64::new(1);

/// The registration's invocation count and whether it has been removed.
#[derive(Default)]
struct State {
    retired: bool,
    in_flight: usize,
}

/// One `keel_port_register` call: the host's callback and pointer, and what is running on them.
pub(crate) struct PortReg {
    cb: KeelPortCb,
    user: UserPtr,
    serial: u64,
    state: Mutex<State>,
    /// Signalled when a retired registration's last invocation ends.
    drained: Condvar,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PortReg {
    fn new(cb: KeelPortCb, user: *mut c_void) -> PortReg {
        PortReg {
            cb,
            user: UserPtr(user),
            serial: SERIALS.fetch_add(1, Ordering::Relaxed),
            state: Mutex::new(State::default()),
            drained: Condvar::new(),
        }
    }

    /// Counts one invocation, unless the registration has been retired.
    fn begin(self: Arc<PortReg>) -> Option<Invocation> {
        {
            let mut state = lock(&self.state);
            if state.retired {
                return None;
            }
            state.in_flight += 1;
        }
        let _ = RUNNING.try_with(|running| running.borrow_mut().push(self.serial));
        Some(Invocation { reg: self })
    }

    /// Stops new invocations from starting. Idempotent.
    fn mark_retired(&self) {
        lock(&self.state).retired = true;
    }

    /// Waits until no invocation started before [`PortReg::mark_retired`] is still running,
    /// except the ones on the calling thread's own stack (which are waiting for us).
    fn drain(&self) {
        let own = RUNNING
            .try_with(|running| {
                running
                    .borrow()
                    .iter()
                    .filter(|serial| **serial == self.serial)
                    .count()
            })
            .unwrap_or(0);
        debug_assert!(
            own == 0,
            "keel-ffi: a port registration was removed from inside its own callback, which \
             cannot wait for itself (see keel.h: keel_port_register / keel_shutdown)"
        );
        let mut state = lock(&self.state);
        while state.in_flight > own {
            state = self
                .drained
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn retire(&self) {
        self.mark_retired();
        self.drain();
    }
}

/// One running callback of a registration. While it lives, removing that registration waits, so
/// the `user` pointer it exposes stays valid.
pub(crate) struct Invocation {
    reg: Arc<PortReg>,
}

impl Invocation {
    /// The host's port callback.
    pub(crate) fn callback(&self) -> KeelPortCb {
        self.reg.cb
    }

    /// The host's `user` pointer for the callback; valid until this invocation is dropped.
    pub(crate) fn user(&self) -> *mut c_void {
        self.reg.user.0
    }
}

impl Drop for Invocation {
    fn drop(&mut self) {
        let _ = RUNNING.try_with(|running| {
            let mut running = running.borrow_mut();
            if let Some(at) = running
                .iter()
                .rposition(|serial| *serial == self.reg.serial)
            {
                running.remove(at);
            }
        });
        let mut state = lock(&self.reg.state);
        state.in_flight -= 1;
        if state.retired {
            self.reg.drained.notify_all();
        }
    }
}

/// Port callbacks by port id.
pub(crate) struct Registry {
    ports: RwLock<BTreeMap<u32, Arc<PortReg>>>,
}

/// The process-wide registry behind `keel_port_register`: it survives between registration and
/// `keel_shutdown`, and a registration made before `keel_init` applies once the runtime is up.
pub(crate) static PORTS: Registry = Registry::new();

impl Registry {
    pub(crate) const fn new() -> Registry {
        Registry {
            ports: RwLock::new(BTreeMap::new()),
        }
    }

    /// Registers `cb` for `port_id`, replacing (and draining) a previous registration.
    pub(crate) fn install(&self, port_id: u32, cb: KeelPortCb, user: *mut c_void) {
        let new = Arc::new(PortReg::new(cb, user));
        let old = self
            .ports
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(port_id, new);
        if let Some(old) = old {
            old.retire();
        }
    }

    /// Removes the registration of `port_id`, returning once none of its callbacks is running.
    pub(crate) fn remove(&self, port_id: u32) {
        let old = self
            .ports
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&port_id);
        if let Some(old) = old {
            old.retire();
        }
    }

    /// Removes every registration (`keel_shutdown`), returning once no callback of any of them
    /// is running.
    pub(crate) fn retire_all(&self) {
        let all: Vec<Arc<PortReg>> =
            std::mem::take(&mut *self.ports.write().unwrap_or_else(PoisonError::into_inner))
                .into_values()
                .collect();
        for reg in &all {
            reg.mark_retired();
        }
        for reg in &all {
            reg.drain();
        }
    }

    /// The registered port ids.
    pub(crate) fn ids(&self) -> Vec<u32> {
        self.ports
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .copied()
            .collect()
    }

    /// Starts an invocation of the callback registered for `port_id`, if there is one.
    pub(crate) fn enter(&self, port_id: u32) -> Option<Invocation> {
        loop {
            let reg = self
                .ports
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&port_id)
                .cloned()?;
            if let Some(invocation) = reg.begin() {
                return Some(invocation);
            }
            // Retired between the lookup and the count. A retired registration has already left
            // the map, so the next lookup sees its replacement or nothing.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buf::KeelBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::time::Duration;

    unsafe extern "C" fn nothing(
        _: *mut c_void,
        _: u32,
        _: u32,
        _: u32,
        _: *const u8,
        _: u32,
        _: *mut KeelBuf,
    ) -> u8 {
        2
    }

    fn registry_with(port: u32) -> Registry {
        let registry = Registry::new();
        registry.install(port, nothing, core::ptr::null_mut());
        registry
    }

    #[test]
    fn an_unregistered_port_cannot_be_entered() {
        let registry = Registry::new();
        assert!(registry.enter(7).is_none());
        let registry = registry_with(7);
        assert!(registry.enter(7).is_some());
        registry.remove(7);
        assert!(registry.enter(7).is_none());
        assert!(registry.ids().is_empty());
    }

    #[test]
    fn removal_waits_for_an_invocation_on_another_thread() {
        let registry = registry_with(7);
        let invocation = registry.enter(7).expect("registered");
        let removed = AtomicBool::new(false);
        let (started_tx, started_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                started_tx.send(()).expect("listening");
                registry.remove(7);
                removed.store(true, Ordering::Release);
            });
            started_rx.recv().expect("the remover started");
            std::thread::sleep(Duration::from_millis(50));
            assert!(
                !removed.load(Ordering::Acquire),
                "removal returned with an invocation running"
            );
            // A retired registration starts nothing new, even before the wait ends.
            assert!(registry.enter(7).is_none());
            drop(invocation);
        });
        assert!(removed.load(Ordering::Acquire));
    }

    #[test]
    fn replacement_waits_for_the_old_registration_and_serves_the_new_one() {
        let registry = registry_with(7);
        let old = registry.enter(7).expect("registered");
        let replaced = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                registry.install(7, nothing, core::ptr::without_provenance_mut(8));
                replaced.store(true, Ordering::Release);
            });
            // The replacement is visible at once, while the old invocation is still running.
            let fresh = loop {
                let candidate = registry.enter(7).expect("always some registration");
                if candidate.user() as usize == 8 {
                    break candidate;
                }
                drop(candidate);
                std::thread::yield_now();
            };
            std::thread::sleep(Duration::from_millis(50));
            assert!(!replaced.load(Ordering::Acquire), "replace did not wait");
            drop(fresh);
            drop(old);
        });
        assert!(replaced.load(Ordering::Acquire));
    }

    #[test]
    fn retire_all_waits_for_every_registration() {
        let registry = Registry::new();
        for port in [1, 2, 3] {
            registry.install(port, nothing, core::ptr::null_mut());
        }
        let held: Vec<Invocation> = [1, 3]
            .iter()
            .map(|port| registry.enter(*port).expect("registered"))
            .collect();
        let done = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                registry.retire_all();
                done.store(true, Ordering::Release);
            });
            std::thread::sleep(Duration::from_millis(50));
            assert!(!done.load(Ordering::Acquire));
            drop(held);
        });
        assert!(done.load(Ordering::Acquire));
        assert!(registry.ids().is_empty());
        assert!(registry.enter(2).is_none());
    }

    #[test]
    fn removing_a_registration_another_thread_does_not_run_does_not_wait_for_unrelated_ones() {
        let registry = Registry::new();
        registry.install(1, nothing, core::ptr::null_mut());
        registry.install(2, nothing, core::ptr::null_mut());
        let busy = registry.enter(1).expect("registered");
        registry.remove(2); // returns at once: nothing of port 2 is running
        drop(busy);
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn a_removal_from_inside_its_own_invocation_does_not_wait_for_it() {
        let registry = registry_with(7);
        let _own = registry.enter(7).expect("registered");
        registry.remove(7); // would deadlock if it waited for the invocation below it on the stack
        assert!(registry.enter(7).is_none());
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "removed from inside its own callback")]
    fn a_removal_from_inside_its_own_invocation_is_a_debug_assertion() {
        let registry = registry_with(7);
        let _own = registry.enter(7).expect("registered");
        registry.remove(7);
    }

    #[test]
    fn invocations_are_counted_per_thread_and_nested_ones_release_in_order() {
        let registry = registry_with(7);
        let outer = registry.enter(7).expect("registered");
        let inner = registry.enter(7).expect("registered");
        assert_eq!(RUNNING.with(|running| running.borrow().len()), 2);
        drop(outer);
        assert_eq!(RUNNING.with(|running| running.borrow().len()), 1);
        drop(inner);
        assert_eq!(RUNNING.with(|running| running.borrow().len()), 0);
    }
}
