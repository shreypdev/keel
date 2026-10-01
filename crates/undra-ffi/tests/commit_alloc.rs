//! The allocation gate of the commit path (harsh-conditions design, decision D4): a counting
//! global allocator proves that one signal write through `Runtime::call_sync_with` allocates
//! **nothing** when no host observes the store, and **at most three times** when one does.
//!
//! The firehose scenario (`bench/common/stress.rs`) pushes 100,000 commits a second, so every
//! allocation on the commit path is 100,000 `malloc`/`free` pairs a second. Today an observed
//! commit makes exactly three (`group_by_store`'s `groups` and `ids` vectors in
//! `keel-signals/src/txn.rs`, and the `claimed` vector in `store.rs`); reaching zero is a roadmap
//! line. This test fails the day a fourth appears, and, like `sync_alloc.rs`, is the only place
//! allowed to count: a global allocator is `unsafe`, and `unsafe` lives in this crate (R2).
//!
//! The write is `update` (in place) through a host that keeps two integers, so the allocations
//! counted are the runtime's and the signals crate's, not the method's (a `Signal::set` boxes
//! its new value) and not a recording host's. The counter is per thread, so other test threads
//! do not disturb the numbers.
#![allow(unsafe_code)]
#![deny(clippy::undocumented_unsafe_blocks)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use keel::meta::ids;
use keel::prelude::Handle;
use keel::runtime::testing::{call_payload, decode_reply};
use keel::runtime::{Host, PortCallOutcome, Runtime, RuntimeConfig};
use keel::signals::ALL_SIGNALS;
use keel::wire::Decode;
use keel::wire::payload::{CallTarget, ReplyStatus};

#[path = "common/core.rs"]
mod test_core;

// ---------------------------------------------------------------------------------------------
// A global allocator that counts the calling thread's allocations
// ---------------------------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------------------------
// A host that only counts
// ---------------------------------------------------------------------------------------------

/// Keeps two integers per change-set and allocates nothing.
#[derive(Default)]
struct CountingHost {
    change_sets: AtomicU64,
    change_set_bytes: AtomicU64,
}

impl Host for CountingHost {
    fn reply(&self, _call_id: u32, _payload: &[u8]) {}

    fn change_set(&self, payload: &[u8]) {
        self.change_sets.fetch_add(1, Ordering::Relaxed);
        self.change_set_bytes
            .fetch_add(payload.len() as u64, Ordering::Relaxed);
    }

    fn stream_item(&self, _call_id: u32, _payload: &[u8]) {}

    fn port_call(&self, _: u32, _: u32, _: u32, _: &[u8]) -> PortCallOutcome {
        PortCallOutcome::Unavailable
    }

    fn log(&self, _level: u8, _target: &str, _message: &str) {}
}

// ---------------------------------------------------------------------------------------------
// The claims
// ---------------------------------------------------------------------------------------------

/// Commits measured per claim.
const COMMITS: usize = 1_000;

/// The most allocations one observed commit may make.
const OBSERVED_LIMIT: usize = 3;

/// A runtime (no threads) holding a `Counter` store, and the prebuilt `bump` call that writes
/// its one signal in place.
fn counter() -> (Arc<Runtime>, Arc<CountingHost>, Handle, Vec<u8>) {
    let host = Arc::new(CountingHost::default());
    let config = RuntimeConfig {
        platform: "alloc-gate".to_owned(),
        core_threads: 0,
        ..RuntimeConfig::default()
    };
    let rt = Runtime::new(config, host.clone()).expect("a runtime");
    let constructor = call_payload(
        CallTarget::Constructor {
            type_id: ids::type_id("Counter"),
            method_id: ids::method_id("Counter", "new"),
        },
        1,
        &[],
    );
    let reply = decode_reply(&rt.call_sync(&constructor));
    assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    let handle = Handle::decode_exact(&reply.body).expect("a handle");
    let bump = call_payload(
        CallTarget::Method {
            handle,
            method_id: ids::method_id("Counter", "bump"),
        },
        2,
        &[],
    );
    (rt, host, handle, bump)
}

/// `COMMITS` writes through the zero-allocation synchronous path, after a warm-up that fills the
/// thread's reply buffer and every lazily built thread-local.
fn commits(rt: &Runtime, bump: &[u8]) -> usize {
    for _ in 0..10 {
        rt.call_sync_with(bump, |_| ());
    }
    allocations_in(|| {
        for _ in 0..COMMITS {
            rt.call_sync_with(bump, |reply| assert_eq!(reply[4], ReplyStatus::Ok.as_u8()));
        }
    })
}

#[test]
fn an_unobserved_commit_allocates_nothing() {
    let (rt, host, _handle, bump) = counter();
    let allocations = commits(&rt, &bump);
    assert_eq!(
        host.change_sets.load(Ordering::Relaxed),
        0,
        "nothing observes the store, so nothing is delivered"
    );
    rt.shutdown();
    assert_eq!(
        allocations, 0,
        "{COMMITS} unobserved commits made {allocations} allocations; a dirty slot nobody \
         observes is never encoded, and the commit path must not touch the heap for it"
    );
}

#[test]
fn an_observed_commit_allocates_at_most_three_times() {
    let (rt, host, handle, bump) = counter();
    rt.observe(handle.0, ALL_SIGNALS, true);
    let (sets, bytes) = (
        host.change_sets.load(Ordering::Relaxed),
        host.change_set_bytes.load(Ordering::Relaxed),
    );
    let allocations = commits(&rt, &bump);
    let delivered = host.change_sets.load(Ordering::Relaxed) - sets;
    let shipped = host.change_set_bytes.load(Ordering::Relaxed) - bytes;
    rt.shutdown();
    // The warm-up commits are delivered too; what matters is that every commit was a real,
    // observed one (a 33-byte change-set: 12 header, 17 entry header, a 4-byte value).
    assert_eq!(
        delivered as usize,
        COMMITS + 10,
        "one change-set per commit"
    );
    assert_eq!(
        shipped,
        delivered * 33,
        "each change-set carries the one entry"
    );
    assert!(
        allocations <= OBSERVED_LIMIT * COMMITS,
        "{COMMITS} observed commits made {allocations} allocations \
         ({:.2} per commit); the limit is {OBSERVED_LIMIT} per commit",
        allocations as f64 / COMMITS as f64
    );
}
