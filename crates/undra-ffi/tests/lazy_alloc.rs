//! Allocation discipline of lazy lists (ADR-043), measured with a counting global allocator (the
//! allocator needs `unsafe`, which constitution R2 keeps in this crate; the code under test is
//! `undra-signals` and `undra-runtime`).
//!
//! * A write to an observed `Lazy<u64>` commits one 12-byte `LazyInvalidated` entry. After warm-up
//!   it allocates exactly **one** buffer more than the same write to an observed plain counter
//!   (the commit's scratch buffer for the entry, reserved at its final size), and **the same number
//!   whether the list has 10 items or 100,000**: nothing walks, clones or encodes the list.
//! * Encoding a page of 50 rows into a buffer that is big enough allocates nothing, whatever the
//!   list holds: a page is the rows it is asked for, copied out.
#![allow(unsafe_code)]
#![deny(clippy::undocumented_unsafe_blocks)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use undra::signals::{ChangeSink, Lazy, LazySource, Signal, StoreCell, with_sink};
use undra::wire::Writer;
use undra::wire::payload::{ChangeSet, LazyInvalidated};

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

/// A sink that checks what it is given and keeps nothing (a host that applies in place).
struct Discard {
    entry_bytes: usize,
}

impl ChangeSink for Discard {
    fn deliver(&self, change_set: &[u8]) {
        // 12 header, 17 entry header, then the value.
        assert_eq!(change_set.len(), 12 + 17 + self.entry_bytes);
    }
}

/// The allocations of the `n`th write after warm-up, and that they are a steady state.
fn steady(mut write: impl FnMut(u64)) -> usize {
    for i in 0..64 {
        write(i);
    }
    let counts: Vec<usize> = (64..80).map(|i| allocations_in(|| write(i))).collect();
    assert!(
        counts.windows(2).all(|w| w[0] == w[1]),
        "a steady state: {counts:?}"
    );
    counts[0]
}

/// Allocations per write to an observed `Lazy<u64>` of `n` items (a push).
fn lazy_push(n: u64) -> usize {
    let cell = StoreCell::new(0xA110C);
    cell.set_handle(0x1_0000_0001);
    let list = Lazy::from_vec((0..n).collect());
    cell.attach_lazy(&list, 0).unwrap();
    cell.observe(0, true, &mut Writer::new());
    let sink: Arc<dyn ChangeSink> = Arc::new(Discard { entry_bytes: 12 });
    with_sink(sink, || steady(|i| list.push(i)))
}

/// Allocations per write to an observed `Lazy<u64>` of `n` items (an in-place update).
fn lazy_update(n: u64) -> usize {
    let cell = StoreCell::new(0xA110C);
    cell.set_handle(0x1_0000_0001);
    let list = Lazy::from_vec((0..n).collect());
    cell.attach_lazy(&list, 0).unwrap();
    cell.observe(0, true, &mut Writer::new());
    let sink: Arc<dyn ChangeSink> = Arc::new(Discard { entry_bytes: 12 });
    with_sink(sink, || {
        steady(|i| list.update_at((i % n) as usize, |v| *v += 1))
    })
}

/// The same shape with a plain observed counter written in place: the baseline.
fn counter() -> usize {
    let cell = StoreCell::new(0xA110C);
    cell.set_handle(0x1_0000_0001);
    let count = Signal::new(0_u64);
    cell.attach(&count, 0).unwrap();
    cell.observe(0, true, &mut Writer::new());
    let sink: Arc<dyn ChangeSink> = Arc::new(Discard { entry_bytes: 8 });
    with_sink(sink, || steady(|_| count.update(|n| *n += 1)))
}

#[test]
fn a_lazy_commit_allocates_its_entry_and_nothing_that_grows_with_the_list() {
    let baseline = counter();
    let small = lazy_update(10);
    let large = lazy_update(100_000);
    let pushes = lazy_push(100_000);
    println!(
        "allocations per write: plain counter {baseline}; Lazy of 10: {small}; Lazy of 100,000: \
         {large}; push to a Lazy of 100,000: {pushes}"
    );
    assert_eq!(
        small, large,
        "the list's length does not reach the commit: it is 12 bytes whatever the list holds"
    );
    assert_eq!(
        pushes, large,
        "an append costs the commit what an update does (a push can grow the vector, which \
         amortises to nothing and happens outside the commit)"
    );
    assert_eq!(
        large,
        baseline + 1,
        "one buffer for the 12-byte entry beyond a plain slot"
    );
}

#[test]
fn a_page_of_fifty_rows_encodes_without_allocating_whatever_the_list_holds() {
    let page_allocations = |n: u64| {
        let list = Lazy::from_vec((0..n).collect::<Vec<u64>>());
        let mut out = Writer::with_capacity(50 * 8);
        // Warm-up: nothing is built lazily, but the first call faults the code in.
        list.encode_page(0, 50, &mut out);
        out.clear();
        let counts: Vec<usize> = (0..8)
            .map(|k| {
                out.clear();
                allocations_in(|| {
                    let header = list.encode_page(k * 50, 50, &mut out);
                    assert_eq!((header.count, out.as_slice().len()), (50, 400));
                })
            })
            .collect();
        counts
    };
    for n in [1_000, 100_000] {
        assert_eq!(
            page_allocations(n),
            [0; 8],
            "encoding a page into a buffer that is big enough allocates nothing, {n} items"
        );
    }
}

#[test]
fn the_invalidation_a_commit_sends_is_the_twelve_bytes_and_nothing_else() {
    // The counts above are only worth anything if the entry really is the 12 bytes.
    #[derive(Default)]
    struct Keep(std::sync::Mutex<Vec<Vec<u8>>>);
    impl ChangeSink for Keep {
        fn deliver(&self, change_set: &[u8]) {
            self.0.lock().unwrap().push(change_set.to_vec());
        }
    }
    let cell = StoreCell::new(0xA110C);
    cell.set_handle(0x1_0000_0001);
    let list = Lazy::from_vec((0..100_000_u64).collect::<Vec<_>>());
    cell.attach_lazy(&list, 0).unwrap();
    cell.observe(0, true, &mut Writer::new());
    let keep = Arc::new(Keep::default());
    let sink: Arc<dyn ChangeSink> = keep.clone();
    with_sink(sink, || list.push(7));
    let sets = keep.0.lock().unwrap();
    assert_eq!(sets.len(), 1);
    let decoded = ChangeSet::decode(&mut undra::wire::Reader::new(&sets[0])).unwrap();
    assert_eq!(decoded.entries[0].value.len(), 12);
    let inv =
        LazyInvalidated::decode(&mut undra::wire::Reader::new(&decoded.entries[0].value)).unwrap();
    assert_eq!((inv.len, inv.version), (100_001, 1));
}
