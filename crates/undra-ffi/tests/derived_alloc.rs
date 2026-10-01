//! Allocation discipline of derived lists (ADR-039), measured with a counting global allocator
//! (the allocator needs `unsafe`, which constitution R2 keeps in this crate; the code under test is
//! `undra-signals`).
//!
//! After warm-up, an `update_at` on a keyed `Signal<Vec<u64>>` with one observed unsorted derived
//! view and no `map` allocates exactly as much as a commit of the same shape without a view (the
//! source and an observed plain counter, written in place): the tap's op buffer, the drain's
//! replay, the pending derived ops and the patch encoding all reuse their buffers, and the item
//! recorded for the tap is moved into the emitted op instead of being cloned again. A second
//! claimed slot costs the commit one allocation of its own, whatever the slot is: the per-store
//! list of dirty slot ids (`group_by_store` in `undra-signals/src/txn.rs`, one of the three vectors
//! `commit_alloc.rs` counts) grows to hold it. (`u64` items own no heap memory, so the counts are
//! of the machinery.)
//!
//! Debug builds check every derived patch against a materialised view and every keyed patch
//! against the list, which allocates per item, so the counts are asserted in release builds
//! (`cargo test -p undra-ffi --release --test derived_alloc`); debug builds check the steady state
//! and the delivery counters only.
#![allow(unsafe_code)]
#![deny(clippy::undocumented_unsafe_blocks)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use undra::signals::{ChangeSink, DerivedList, Signal, StoreCell, with_sink};
use undra::wire::Writer;

struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn bump() {
    let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
}

// SAFETY: every method forwards to `System` unchanged; the only addition is a counter in a
// `const`-initialised, destructor-free thread-local, which never allocates.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump();
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract; we pass it through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump();
        // SAFETY: as in `alloc`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump();
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract; we pass it through.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `GlobalAlloc::dealloc`'s contract; we pass it through.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// How many allocations this thread made while `f` ran.
fn allocations_in(f: impl FnOnce()) -> usize {
    let before = ALLOCATIONS.with(Cell::get);
    f();
    ALLOCATIONS.with(Cell::get) - before
}

/// A sink that looks at the change-set and keeps nothing (a host that applies in place).
struct Discard;

impl ChangeSink for Discard {
    fn deliver(&self, change_set: &[u8]) {
        assert!(change_set.len() > 12, "a change-set with entries");
    }
}

/// A store with a keyed source of 10,000 `u64`s, observed, and optionally an observed unsorted
/// view of the even ones; returns the allocations of the `n`th `update_at` after warm-up.
#[derive(Clone, Copy, PartialEq)]
enum View {
    /// The keyed source alone.
    None,
    /// A view attached but never observed: dirtied once, never drained or encoded.
    Unobserved,
    /// An observed view: two claimed slots.
    Observed,
    /// No view, an observed plain counter written in place in the same transaction: two claimed
    /// slots, the baseline of the same shape.
    Counter,
}

fn allocations_per_update(mode: View) -> usize {
    let cell = StoreCell::new(0xA110C);
    cell.set_handle(0x1_0000_0001);
    let rows = Signal::new((0..10_000_u64).collect::<Vec<_>>());
    cell.attach_keyed(&rows, 0, |n: &u64| *n).unwrap();
    let view: Option<DerivedList<u64>> =
        matches!(mode, View::Unobserved | View::Observed).then(|| {
            let view = rows.derive().filter(|n: &u64| n % 2 == 0).build();
            cell.attach_derived(&view, 1, |n: &u64| *n).unwrap();
            view
        });
    let counter = Signal::new(0_u64);
    if mode == View::Counter {
        cell.attach(&counter, 1).unwrap();
    }
    cell.observe(0, true, &mut Writer::new());
    if matches!(mode, View::Observed | View::Counter) {
        cell.observe(1, true, &mut Writer::new());
    }
    let sink: Arc<dyn ChangeSink> = Arc::new(Discard);
    with_sink(sink, || {
        let write = |i: u64| {
            undra::signals::txn(|| {
                rows.update_at((i * 2) as usize % 10_000, |n| *n = i * 2);
                if mode == View::Counter {
                    counter.update(|n| *n += 1);
                }
            });
        };
        for i in 0..64 {
            write(i);
        }
        let counts: Vec<usize> = (64..80).map(|i| allocations_in(|| write(i))).collect();
        assert!(
            counts.windows(2).all(|w| w[0] == w[1]),
            "a steady state: {counts:?}"
        );
        if let (View::Observed, Some(view)) = (mode, &view) {
            assert_eq!(view.stats().full_values, 1, "the observe only");
            assert_eq!(view.stats().patches, 80, "every write was one patch");
            assert_eq!(view.stats().rebuilds, 1, "the observe only");
        }
        counts[0]
    })
}

#[test]
fn a_derived_view_adds_no_allocation_to_a_recorded_write() {
    let source = allocations_per_update(View::None);
    let unobserved = allocations_per_update(View::Unobserved);
    let observed = allocations_per_update(View::Observed);
    let counter = allocations_per_update(View::Counter);
    println!(
        "allocations per update_at: keyed source {source}; + unobserved view {unobserved}; \
         + observed view {observed}; + observed counter (same shape, no view) {counter}"
    );
    if cfg!(debug_assertions) {
        return;
    }
    assert_eq!(
        observed, counter,
        "the view's tap, drain, pending ops and patch reuse their buffers"
    );
    assert_eq!(
        unobserved, source,
        "an unobserved view costs a write nothing"
    );
    assert_eq!(
        counter,
        source + 1,
        "a second claimed slot grows the commit's id list once"
    );
}
