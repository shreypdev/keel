//! The C ABI's port registrations (SPEC 6.3) and the rule that keeps a host's `user` pointer valid
//! for exactly as long as a callback can still use it (ADR-026).
//!
//! `undra_port_register` hands the core a `(callback, user)` pair. The host is allowed to free
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

use crate::native::UndraPortCb;

/// An opaque host pointer handed back to its callbacks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct UserPtr(pub(crate) *mut c_void);

// SAFETY: the pointer is never dereferenced here, only passed back to the host's own callbacks.
// `undra.h` makes the host responsible for those callbacks being callable from any thread.
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

/// One `undra_port_register` call: the host's callback and pointer, and what is running on them.
pub(crate) struct PortReg {
    cb: UndraPortCb,
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
    fn new(cb: UndraPortCb, user: *mut c_void) -> PortReg {
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
            "undra-ffi: a port registration was removed from inside its own callback, which \
             cannot wait for itself (see undra.h: undra_port_register / undra_shutdown)"
        );
        let mut state = lock(&self.state);
        while state.in_flight > own {
            state = self
                .drained
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// One running callback of a registration. While it lives, removing that registration waits, so
/// the `user` pointer it exposes stays valid.
pub(crate) struct Invocation {
    reg: Arc<PortReg>,
}

impl Invocation {
    /// The host's port callback.
    pub(crate) fn callback(&self) -> UndraPortCb {
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
    /// Registrations taken out of `ports` whose callbacks may still be running. Every removal
    /// of a port id waits for ALL entries of that id here — not only the one it took out of
    /// the map itself — so the loser of a removal race still returns only once no callback of
    /// that port runs (re-review N1). Entries leave the list once drained.
    draining: Mutex<Vec<(u32, Arc<PortReg>)>>,
}

/// The process-wide registry behind `undra_port_register`: it survives between registration and
/// `undra_shutdown`, and a registration made before `undra_init` applies once the runtime is up.
pub(crate) static PORTS: Registry = Registry::new();

impl Registry {
    pub(crate) const fn new() -> Registry {
        Registry {
            ports: RwLock::new(BTreeMap::new()),
            draining: Mutex::new(Vec::new()),
        }
    }

    /// Registers `cb` for `port_id`, replacing (and draining) a previous registration.
    pub(crate) fn install(&self, port_id: u32, cb: UndraPortCb, user: *mut c_void) {
        let new = Arc::new(PortReg::new(cb, user));
        let old = self
            .ports
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(port_id, new);
        self.settle(Some(port_id), old.map(|old| (port_id, old)).into_iter());
    }

    /// Removes the registration of `port_id`, returning once none of its callbacks is running —
    /// including callbacks of a registration that a concurrent removal or shutdown took out of
    /// the map first (re-review N1).
    pub(crate) fn remove(&self, port_id: u32) {
        let old = self
            .ports
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&port_id);
        self.settle(Some(port_id), old.map(|old| (port_id, old)).into_iter());
    }

    /// Removes every registration (`undra_shutdown`), returning once no callback of any of them
    /// is running, whoever took them out of the map.
    pub(crate) fn retire_all(&self) {
        let all: Vec<(u32, Arc<PortReg>)> =
            std::mem::take(&mut *self.ports.write().unwrap_or_else(PoisonError::into_inner))
                .into_iter()
                .collect();
        self.settle(None, all.into_iter());
    }

    /// Publishes `taken` on the draining list, retires the entries, then waits until no
    /// callback of `port_id` (every port for `None`) still runs — the entries taken here AND
    /// the ones concurrent removals published. Skips all waiting, with a debug assertion and a
    /// FATAL log, when called from inside a port callback (undra.h forbids it; waiting could
    /// only deadlock — re-review N2).
    fn settle(&self, port_id: Option<u32>, taken: impl Iterator<Item = (u32, Arc<PortReg>)>) {
        {
            let mut draining = self.draining.lock().unwrap_or_else(PoisonError::into_inner);
            for (id, reg) in taken {
                reg.mark_retired();
                draining.push((id, reg));
            }
        }
        let on_callback_thread = RUNNING
            .try_with(|running| !running.borrow().is_empty())
            .unwrap_or(false);
        if on_callback_thread {
            // The own-registration case keeps its historical assertion message; drain() below
            // would raise it. Anything else is the mutual-removal deadlock of re-review N2.
            let own_only = RUNNING.try_with(|running| {
                let running = running.borrow();
                let draining = self.draining.lock().unwrap_or_else(PoisonError::into_inner);
                running.iter().all(|serial| {
                    draining
                        .iter()
                        .any(|(id, reg)| reg.serial == *serial && port_id.is_none_or(|p| *id == p))
                })
            });
            debug_assert!(
                own_only.unwrap_or(true),
                "undra-ffi: undra_port_register / undra_shutdown must not be called from inside a \
                 port callback (see undra.h); waiting here would deadlock"
            );
            #[cfg(debug_assertions)]
            {
                // Drain an entry whose callback is on THIS thread's stack: that is the one
                // whose assertion ("removed from inside its own callback") should fire, and
                // it cannot wait on anything.
                let own = RUNNING
                    .try_with(|running| running.borrow().clone())
                    .unwrap_or_default();
                let picked = {
                    let draining = self.draining.lock().unwrap_or_else(PoisonError::into_inner);
                    draining
                        .iter()
                        .find(|(_, reg)| own.contains(&reg.serial))
                        .map(|(_, reg)| reg.clone())
                };
                if let Some(reg) = picked {
                    reg.drain();
                }
            }
            if let Some(rt) = undra_runtime::Runtime::global() {
                rt.log(
                    undra_runtime::log::FATAL,
                    "undra::ffi",
                    "undra_port_register / undra_shutdown called from inside a port callback; the \
                     removal returns WITHOUT waiting and the `user` pointer may still be in use",
                );
            }
            return;
        }
        // Outlast every draining entry of the id(s); entries whose drain completed leave the
        // list, so a violating host's leftovers are cleaned up by the next legitimate removal.
        while let Some(reg) = self.pick_draining(port_id) {
            reg.drain();
            let mut draining = self.draining.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(at) = draining.iter().position(|(_, r)| Arc::ptr_eq(r, &reg)) {
                draining.remove(at);
            }
        }
    }

    /// One draining entry matching `port_id` (any entry for `None`), if there is one.
    fn pick_draining(&self, port_id: Option<u32>) -> Option<Arc<PortReg>> {
        self.draining
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(id, _)| port_id.is_none_or(|p| *id == p))
            .map(|(_, reg)| reg.clone())
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
    use crate::buf::UndraBuf;
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
        _: *mut UndraBuf,
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
        let (held_tx, held_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let holder = &registry;
            scope.spawn(move || {
                let busy = holder.enter(1).expect("registered");
                held_tx.send(()).expect("listening");
                done_rx.recv().expect("released");
                drop(busy);
            });
            held_rx.recv().expect("an invocation of port 1 is running");
            registry.remove(2); // returns at once: nothing of port 2 is running
            done_tx.send(()).expect("the holder is waiting");
        });
    }

    /// Re-review N1: the loser of a removal race must still wait. Whoever took the
    /// registration out of the map, every `remove` / `retire_all` of that id returns only
    /// once no callback of the id is running.
    #[test]
    fn n1_the_loser_of_a_removal_race_still_waits_for_the_callback() {
        let registry = registry_with(7);
        let invocation = registry.enter(7).expect("registered");
        let first = AtomicBool::new(false);
        let second = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                registry.retire_all(); // takes the registration out of the map
                first.store(true, Ordering::Release);
            });
            // Make sure the shutdown grabbed it before the losing remover runs.
            while !registry.ids().is_empty() {
                std::thread::yield_now();
            }
            scope.spawn(|| {
                registry.remove(7); // finds nothing in the map; must wait regardless
                second.store(true, Ordering::Release);
            });
            std::thread::sleep(Duration::from_millis(50));
            assert!(!first.load(Ordering::Acquire), "shutdown returned early");
            assert!(
                !second.load(Ordering::Acquire),
                "the losing remover returned early"
            );
            drop(invocation);
        });
        assert!(first.load(Ordering::Acquire));
        assert!(second.load(Ordering::Acquire));
    }

    /// Re-review N1, both callers `remove`: same rule.
    #[test]
    fn n1_two_removers_of_one_port_both_wait() {
        let registry = registry_with(7);
        let invocation = registry.enter(7).expect("registered");
        let done = [AtomicBool::new(false), AtomicBool::new(false)];
        std::thread::scope(|scope| {
            for flag in &done {
                let registry = &registry;
                scope.spawn(move || {
                    registry.remove(7);
                    flag.store(true, Ordering::Release);
                });
            }
            std::thread::sleep(Duration::from_millis(50));
            assert!(done.iter().all(|f| !f.load(Ordering::Acquire)));
            drop(invocation);
        });
        assert!(done.iter().all(|f| f.load(Ordering::Acquire)));
    }

    /// Re-review N2 (release): a removal from a thread that is inside SOME port callback does
    /// not wait (waiting could deadlock against another callback removing this one); undra.h
    /// forbids the call outright and debug builds assert.
    #[test]
    #[cfg(not(debug_assertions))]
    fn n2_a_removal_from_inside_another_ports_callback_does_not_wait() {
        let registry = Registry::new();
        registry.install(1, nothing, core::ptr::null_mut());
        registry.install(2, nothing, core::ptr::null_mut());
        let _inside = registry.enter(1).expect("registered"); // this thread is "in" port 1
        let (held_tx, held_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let holder = &registry;
            scope.spawn(move || {
                let busy = holder.enter(2).expect("registered");
                held_tx.send(()).expect("listening");
                done_rx.recv().expect("released");
                drop(busy);
            });
            held_rx.recv().expect("an invocation of port 2 is running");
            registry.remove(2); // must return, not deadlock, even though port 2 is running
            assert!(registry.enter(2).is_none(), "still retired");
            done_tx.send(()).expect("the holder is waiting");
        });
    }

    /// Re-review N2 (debug): the same situation is an assertion.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "must not be called from inside a port callback")]
    fn n2_a_removal_from_inside_another_ports_callback_is_a_debug_assertion() {
        let registry = Registry::new();
        registry.install(1, nothing, core::ptr::null_mut());
        registry.install(2, nothing, core::ptr::null_mut());
        let _inside = registry.enter(1).expect("registered");
        registry.remove(2);
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
