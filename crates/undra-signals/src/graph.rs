//! The reactive graph plumbing shared by signals, computeds and effects.
//!
//! Edges point from a source to its *dependents* and are held weakly, so a dropped computed
//! or effect simply stops being notified and its dead entry is pruned the next time the list
//! is walked. A write walks the graph once, marking every downstream node stale
//! ([`Reactive::invalidate`]); nothing user-supplied runs during that walk, which keeps it
//! safe to do while holding locks.
//!
//! Every node also records itself in the current transaction when it is bound to a store slot
//! ([`record`]); that is what makes commit cost proportional to the number of dirty slots.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use parking_lot::Mutex;

use crate::store::StoreCell;

/// A node that must be told when something it reads has changed.
///
/// Not part of the public API; it only appears in signatures of hidden plumbing methods.
pub trait Reactive: Send + Sync {
    /// Something upstream changed. `pass` identifies the write that is walking the graph, so a
    /// node reachable through several paths (a diamond) is processed once per write.
    ///
    /// Must not run user code and must be cheap.
    fn invalidate(self: Arc<Self>, pass: u64);
}

/// The weak dependents of one node.
pub(crate) type Dependents = Mutex<Vec<Weak<dyn Reactive>>>;

static PASS: AtomicU64 = AtomicU64::new(1);

/// Hands out a fresh identifier for one invalidation walk.
pub(crate) fn next_pass() -> u64 {
    PASS.fetch_add(1, Ordering::Relaxed)
}

/// Registers `dependent` on a node's dependents list.
///
/// Dead entries are swept when the list is about to grow, which keeps the cost amortised O(1)
/// and stops a long-lived signal from accumulating dead weak pointers as short-lived computeds
/// come and go.
pub(crate) fn add_dependent(list: &Dependents, dependent: Weak<dyn Reactive>) {
    let mut list = list.lock();
    if list.len() == list.capacity() {
        list.retain(|w| w.strong_count() > 0);
    }
    list.push(dependent);
}

/// Starts an invalidation walk from a node that has just changed. Does nothing (and takes no
/// pass number) when the node has no dependents.
pub(crate) fn notify_dependents(list: &Dependents) {
    let mut list = list.lock();
    if list.is_empty() {
        return;
    }
    let pass = next_pass();
    walk(&mut list, pass);
}

/// Continues an invalidation walk into the dependents of `list`.
pub(crate) fn propagate(list: &Dependents, pass: u64) {
    let mut list = list.lock();
    walk(&mut list, pass);
}

fn walk(list: &mut Vec<Weak<dyn Reactive>>, pass: u64) {
    // Locks are taken parent before child, and a node can only depend on nodes created before
    // it, so the order is consistent and cannot deadlock.
    list.retain(|weak| match weak.upgrade() {
        Some(node) => {
            node.invalidate(pass);
            true
        }
        None => false,
    });
}

/// Per-slot state shared between a [`StoreCell`] and the signal that feeds the slot.
#[derive(Debug, Default)]
pub(crate) struct SlotFlags {
    /// Set by the first write that has not been delivered yet. Whoever flips it from `false`
    /// to `true` records the slot in its transaction and so owns its delivery; later writes
    /// see it set and skip all bookkeeping.
    pub(crate) dirty: AtomicBool,
    /// The host observes this slot, so its changes are delivered.
    pub(crate) observed: AtomicBool,
    /// Deliver changes even while unobserved.
    pub(crate) no_coalesce: AtomicBool,
    /// A computed slot whose last evaluation at a commit or an observe panicked: it is held back
    /// (the host keeps the last value it received) until an evaluation succeeds, which is tried
    /// again when its inputs change (ADR-019 amendment).
    pub(crate) failed: AtomicBool,
}

/// Where a signal or computed lives inside a store.
pub(crate) struct Binding {
    pub(crate) cell: Weak<StoreCell>,
    pub(crate) signal_id: u32,
    pub(crate) flags: Arc<SlotFlags>,
    /// The store's owning runtime (shared with the cell, `0` until it is published): what a write
    /// is checked against (ADR-035), read without reaching the cell.
    pub(crate) owner: Arc<AtomicU64>,
}

/// Notes in the current transaction that the slot behind `binding` changed.
pub(crate) fn record(binding: &Binding) {
    if binding.flags.dirty.swap(true, Ordering::AcqRel) {
        // Already pending: an earlier write owns the delivery.
        return;
    }
    if let Some(cell) = binding.cell.upgrade() {
        crate::txn::push_write(cell, binding.signal_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct Counter {
        hits: AtomicUsize,
        last_pass: AtomicU64,
    }

    impl Reactive for Counter {
        fn invalidate(self: Arc<Self>, pass: u64) {
            self.hits.fetch_add(1, Ordering::SeqCst);
            self.last_pass.store(pass, Ordering::SeqCst);
        }
    }

    fn counter() -> Arc<Counter> {
        Arc::new(Counter {
            hits: AtomicUsize::new(0),
            last_pass: AtomicU64::new(0),
        })
    }

    fn weak(c: &Arc<Counter>) -> Weak<dyn Reactive> {
        let w: Weak<dyn Reactive> = Arc::downgrade(c) as Weak<dyn Reactive>;
        w
    }

    #[test]
    fn notify_invalidates_live_dependents() {
        let list: Dependents = Mutex::new(Vec::new());
        let a = counter();
        let b = counter();
        add_dependent(&list, weak(&a));
        add_dependent(&list, weak(&b));
        notify_dependents(&list);
        assert_eq!(a.hits.load(Ordering::SeqCst), 1);
        assert_eq!(b.hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            a.last_pass.load(Ordering::SeqCst),
            b.last_pass.load(Ordering::SeqCst),
            "one walk, one pass number"
        );
    }

    #[test]
    fn each_walk_gets_a_new_pass() {
        let list: Dependents = Mutex::new(Vec::new());
        let a = counter();
        add_dependent(&list, weak(&a));
        notify_dependents(&list);
        let first = a.last_pass.load(Ordering::SeqCst);
        notify_dependents(&list);
        assert!(a.last_pass.load(Ordering::SeqCst) > first);
    }

    #[test]
    fn dead_dependents_are_pruned_by_a_walk() {
        let list: Dependents = Mutex::new(Vec::new());
        let a = counter();
        {
            let gone = counter();
            add_dependent(&list, weak(&gone));
        }
        add_dependent(&list, weak(&a));
        assert_eq!(list.lock().len(), 2);
        notify_dependents(&list);
        assert_eq!(list.lock().len(), 1);
        assert_eq!(a.hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dead_dependents_are_swept_when_the_list_grows() {
        let list: Dependents = Mutex::new(Vec::new());
        for _ in 0..1000 {
            let temp = counter();
            add_dependent(&list, weak(&temp));
        }
        assert!(
            list.lock().len() < 100,
            "dead weak pointers must not pile up, got {}",
            list.lock().len()
        );
    }

    #[test]
    fn slot_flags_start_clear() {
        let flags = SlotFlags::default();
        assert!(!flags.dirty.load(Ordering::SeqCst));
        assert!(!flags.observed.load(Ordering::SeqCst));
        assert!(!flags.no_coalesce.load(Ordering::SeqCst));
    }
}
