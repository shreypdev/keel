//! Unit tests of derived lists that need the crate's internals: the taps a list signal keeps, and
//! how recorded and raw writes reach them.

use std::sync::atomic::{AtomicBool, Ordering};

use super::tap::TapList;
use super::*;

/// The tap list of a list signal (created by the first `build`).
fn taps_of<I: SignalValue>(signal: &Signal<Vec<I>>) -> &TapList<I> {
    signal.tap_list()
}

#[test]
fn a_raw_write_invalidates_the_taps_and_a_recorded_one_does_not() {
    let s = Signal::new(vec![1_u32, 2]);
    let view = s.derive().build();
    assert_eq!(view.len(), 2, "the first read arms the tap");
    s.push(3);
    s.insert(0, 0);
    s.remove(1);
    s.update_at(0, |n| *n += 10);
    s.move_item(0, 1);
    s.clear();
    assert_eq!(view.stats().rebuilds, 1, "recorded operations replay");
    assert!(view.is_empty());
    s.update(|v| v.push(1));
    assert_eq!(view.get(), [1]);
    assert_eq!(view.stats().rebuilds, 2, "update makes the tap stale");
    s.set(vec![5]);
    assert_eq!(view.get(), [5]);
    s.replace(vec![6]);
    assert_eq!(view.get(), [6]);
    assert_eq!(view.stats().rebuilds, 4, "so do set and replace");
}

#[test]
fn a_new_list_records_nothing_until_it_is_read() {
    let s = Signal::new(vec![1_u32]);
    let view = s.derive().build();
    for n in 0..10 {
        s.push(n);
    }
    assert_eq!(view.stats().rebuilds, 0);
    assert_eq!(view.len(), 11, "the first read builds from the list");
    assert_eq!(view.stats().rebuilds, 1);
}

#[test]
fn two_views_of_one_source_both_see_every_op() {
    let s = Signal::new(vec![1_u32, 2, 3, 4]);
    let odd = s.derive().filter(|n| n % 2 == 1).build();
    let desc = s.derive().sort_by_key(|n| u32::MAX - n).build();
    assert_eq!((odd.len(), desc.len()), (2, 4));
    s.push(5);
    s.update_at(0, |n| *n = 8);
    s.remove(1);
    assert_eq!(odd.get(), [3, 5]);
    assert_eq!(desc.get(), [8, 5, 4, 3]);
    assert_eq!((odd.stats().rebuilds, desc.stats().rebuilds), (1, 1));
}

#[test]
fn a_dropped_view_is_pruned_from_the_tap_list() {
    let s = Signal::new(vec![1_u32]);
    let keep = s.derive().build();
    for _ in 0..20 {
        let gone = s.derive().build();
        assert_eq!(gone.len(), 1);
    }
    assert!(
        taps_of(&s).registered() < 20,
        "dead taps are swept as the list grows"
    );
    s.push(2);
    assert_eq!(keep.get(), [1, 2]);
}

/// An item whose `Clone` panics while armed.
#[derive(Debug)]
struct Fragile(u32, Arc<AtomicBool>);

impl Clone for Fragile {
    fn clone(&self) -> Self {
        assert!(!self.1.load(Ordering::SeqCst), "clone failure");
        Fragile(self.0, Arc::clone(&self.1))
    }
}

impl undra_wire::Encode for Fragile {
    fn encode(&self, w: &mut undra_wire::Writer) {
        self.0.encode(w);
    }
}

#[test]
fn a_panicking_clone_in_a_recorded_operation_makes_every_tap_stale() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let armed = Arc::new(AtomicBool::new(false));
    let s = Signal::new(vec![Fragile(1, Arc::clone(&armed))]);
    let a = s.derive().map(|f: &Fragile| f.0).build();
    let b = s.derive().map(|f: &Fragile| f.0 * 10).build();
    assert_eq!((a.len(), b.len()), (1, 1));
    // The view's map reads the item without cloning it; the tap's op is what clones.
    armed.store(true, Ordering::SeqCst);
    let pushed = catch_unwind(AssertUnwindSafe(|| s.push(Fragile(2, Arc::clone(&armed)))));
    assert!(pushed.is_err());
    armed.store(false, Ordering::SeqCst);
    // The item went in (the list, a slot log and the host agree); both views rebuild.
    assert_eq!(s.with(Vec::len), 2);
    assert_eq!(a.get(), [1, 2]);
    assert_eq!(b.get(), [10, 20]);
    assert_eq!((a.stats().rebuilds, b.stats().rebuilds), (2, 2));
    armed.store(true, Ordering::SeqCst);
    let updated = catch_unwind(AssertUnwindSafe(|| s.update_at(0, |f| f.0 = 7)));
    assert!(updated.is_err());
    armed.store(false, Ordering::SeqCst);
    assert_eq!(a.get(), [7, 2], "the update the closure made is kept");
    assert_eq!(b.get(), [70, 20]);
    assert_eq!((a.stats().rebuilds, b.stats().rebuilds), (3, 3));
}

#[test]
fn a_panicking_update_at_closure_makes_the_taps_stale() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let s = Signal::new(vec![1_u32, 2]);
    let view = s.derive().filter(|n| *n > 1).build();
    assert_eq!(view.get(), [2]);
    let result = catch_unwind(AssertUnwindSafe(|| {
        s.update_at(0, |n| {
            *n = 5;
            panic!("boom");
        })
    }));
    assert!(result.is_err());
    assert_eq!(view.get(), [5, 2], "the change the closure made is seen");
    assert_eq!(view.stats().rebuilds, 2);
}

#[test]
fn derived_lists_are_send_sync_and_clone_shares() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DerivedList<u32>>();
    assert_send_sync::<Derive<u32, String, Sorted<String>>>();
    let s = Signal::new(vec![1_u32]);
    let a = s.derive().build();
    let b = a.clone();
    assert!(a.ptr_eq(&b));
    assert!(!a.ptr_eq(&s.derive().build()));
    let text = format!("{a:?}");
    assert!(
        text.contains("DerivedList") && text.contains("rebuilds: 0"),
        "{text}"
    );
}
