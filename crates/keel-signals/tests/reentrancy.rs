//! Re-entrant writes and panics inside user code that runs during a commit.

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use common::*;
use keel_signals::testing::CaptureSink;
use keel_signals::{ChangeSink, Computed, Signal, StoreCell, txn, with_sink};
use keel_wire::payload::ChangeSet;
use keel_wire::{Encode, Writer};
use parking_lot::Mutex;

/// A store with `a` (0) and `b` (1), both observed.
struct Two {
    rig: Rig,
    a: Signal<u32>,
    b: Signal<u32>,
}

fn two() -> Two {
    let rig = Rig::new();
    let a = Signal::new(0_u32);
    let b = Signal::new(0_u32);
    rig.cell.attach(&a, 0).unwrap();
    rig.cell.attach(&b, 1).unwrap();
    rig.observe_all();
    Two { rig, a, b }
}

/// Records deliveries, asserts they never overlap, and runs a callback for each.
struct Probe {
    capture: Arc<CaptureSink>,
    active: AtomicUsize,
    on_deliver: Box<dyn Fn(&ChangeSet) + Send + Sync>,
}

impl Probe {
    fn new(on_deliver: impl Fn(&ChangeSet) + Send + Sync + 'static) -> Arc<Probe> {
        Arc::new(Probe {
            capture: CaptureSink::new(),
            active: AtomicUsize::new(0),
            on_deliver: Box::new(on_deliver),
        })
    }
}

impl ChangeSink for Probe {
    fn deliver(&self, change_set: &[u8]) {
        struct Leave<'a>(&'a AtomicUsize);
        impl Drop for Leave<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        assert_eq!(
            self.active.fetch_add(1, Ordering::SeqCst),
            0,
            "deliveries must never nest"
        );
        let _leave = Leave(&self.active);
        self.capture.deliver(change_set);
        let decoded = keel_signals::testing::decode(change_set).expect("valid change-set");
        (self.on_deliver)(&decoded);
    }
}

// ---------------------------------------------------------------------------------------------
// writes from a sink
// ---------------------------------------------------------------------------------------------

#[test]
fn a_write_from_the_sink_is_queued_and_committed_as_a_new_transaction() {
    let t = two();
    let b = t.b.clone();
    let probe = Probe::new(move |set| {
        if ids(set) == vec![0] {
            b.set(100);
        }
    });
    with_sink(probe.clone(), || t.a.set(1));
    let sets = probe.capture.take_decoded();
    assert_eq!(sets.len(), 2, "the sink's write becomes its own change-set");
    assert_eq!(ids(&sets[0]), vec![0]);
    assert_eq!(ids(&sets[1]), vec![1]);
    assert_eq!(value_of::<u32>(&sets[1].entries[0]), 100);
    assert!(sets[1].txn_id > sets[0].txn_id);
    assert_eq!(t.b.get(), 100);
}

#[test]
fn a_transaction_opened_by_the_sink_is_one_queued_transaction() {
    let t = two();
    let (a, b) = (t.a.clone(), t.b.clone());
    let probe = Probe::new(move |set| {
        if ids(set) == vec![0] && value_of::<u32>(&set.entries[0]) == 1 {
            txn(|| {
                b.set(5);
                a.set(2);
                b.set(6);
            });
        }
    });
    with_sink(probe.clone(), || t.a.set(1));
    let sets = probe.capture.take_decoded();
    assert_eq!(sets.len(), 2);
    assert_eq!(ids(&sets[1]), vec![0, 1]);
    assert_eq!(value_of::<u32>(entry(&sets[1], 0)), 2);
    assert_eq!(value_of::<u32>(entry(&sets[1], 1)), 6);
}

#[test]
fn a_sink_write_to_an_unobserved_signal_adds_no_change_set() {
    let t = two();
    t.rig.observe_off(1);
    let b = t.b.clone();
    let probe = Probe::new(move |_| b.set(9));
    with_sink(probe.clone(), || t.a.set(1));
    assert_eq!(probe.capture.take_decoded().len(), 1);
    assert_eq!(t.b.get(), 9);
}

#[test]
fn the_sink_can_read_signals_and_computeds() {
    let rig = Rig::new();
    let a = Signal::new(2_u32);
    let doubled = Computed::new(&a, |v| v * 2);
    rig.cell.attach(&a, 0).unwrap();
    rig.observe_all();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (a2, d2, s2) = (a.clone(), doubled.clone(), seen.clone());
    let probe = Probe::new(move |_| s2.lock().push((a2.get(), d2.get())));
    with_sink(probe, || a.set(21));
    assert_eq!(*seen.lock(), vec![(21, 42)]);
}

#[test]
fn the_sink_can_change_observation() {
    let t = two();
    let cell = t.rig.cell.clone();
    let probe = Probe::new(move |set| {
        if ids(set) == vec![0] {
            let mut out = Writer::new();
            cell.observe(1, false, &mut out);
        }
    });
    with_sink(probe.clone(), || t.a.set(1));
    with_sink(probe.clone(), || t.b.set(1));
    let sets = probe.capture.take_decoded();
    assert_eq!(sets.len(), 1, "b was switched off by the sink");
}

#[test]
fn a_sink_that_always_writes_back_is_cut_off_instead_of_looping_forever() {
    let t = two();
    let a = t.a.clone();
    let probe = Probe::new(move |_| a.update(|v| *v += 1));
    with_sink(probe.clone(), || t.a.set(1));
    let delivered = probe.capture.len();
    assert!(delivered > 10, "the loop ran ({delivered} deliveries)");
    assert!(
        delivered <= 1001,
        "and was bounded ({delivered} deliveries)"
    );
}

#[test]
fn a_write_from_inside_update_to_another_signal_joins_the_transaction() {
    let t = two();
    let b = t.b.clone();
    t.rig.run(|| {
        t.a.update(|v| {
            *v = 1;
            b.set(2);
        })
    });
    let set = t.rig.one_set();
    assert_eq!(ids(&set), vec![0, 1]);
}

// ---------------------------------------------------------------------------------------------
// writes from computed closures
// ---------------------------------------------------------------------------------------------

#[test]
fn a_computed_closure_that_writes_is_queued_while_the_commit_recomputes_it() {
    let rig = Rig::new();
    let src = Signal::new(1_u32);
    let log = Signal::new(0_u32);
    let log2 = log.clone();
    let derived = Computed::new(&src, move |v: &u32| {
        log2.set(*v * 100);
        v + 1
    });
    rig.cell.attach(&src, 0).unwrap();
    rig.cell.attach_computed(&derived, 1).unwrap();
    rig.cell.attach(&log, 2).unwrap();
    rig.observe_all();
    rig.run(|| src.set(2));
    let sets = rig.sets();
    assert_eq!(sets.len(), 2);
    assert_eq!(ids(&sets[0]), vec![0, 1]);
    assert_eq!(ids(&sets[1]), vec![2]);
    assert_eq!(value_of::<u32>(&sets[1].entries[0]), 200);
}

#[test]
fn a_lazy_computed_that_writes_commits_immediately() {
    let rig = Rig::new();
    let src = Signal::new(1_u32);
    let log = Signal::new(0_u32);
    let log2 = log.clone();
    let derived = Computed::new(&src, move |v: &u32| {
        log2.set(*v);
        *v
    });
    rig.cell.attach(&log, 0).unwrap();
    rig.observe_all();
    rig.run(|| {
        assert_eq!(derived.get(), 1);
    });
    let set = rig.one_set();
    assert_eq!(value_of::<u32>(&set.entries[0]), 1);
}

// ---------------------------------------------------------------------------------------------
// panics
// ---------------------------------------------------------------------------------------------

#[test]
fn a_panic_in_a_transaction_still_commits_the_writes_already_applied() {
    let t = two();
    let result = catch_unwind(AssertUnwindSafe(|| {
        t.rig.run(|| {
            txn(|| {
                t.a.set(1);
                panic!("boom");
            });
        });
    }));
    assert!(result.is_err());
    let set = t.rig.one_set();
    assert_eq!(
        value_of::<u32>(entry(&set, 0)),
        1,
        "the applied write reached the host"
    );
    // The thread is usable again: the next write commits normally.
    t.rig.run(|| t.b.set(2));
    assert_eq!(ids(&t.rig.one_set()), vec![1]);
}

#[test]
fn a_panic_in_a_nested_transaction_caught_by_the_outer_one_keeps_the_transaction_open() {
    let t = two();
    t.rig.run(|| {
        txn(|| {
            t.a.set(1);
            let inner = catch_unwind(AssertUnwindSafe(|| {
                txn(|| {
                    t.b.set(2);
                    panic!("inner");
                });
            }));
            assert!(inner.is_err());
            assert!(t.rig.sink.is_empty(), "still inside the outer transaction");
        });
    });
    let set = t.rig.one_set();
    assert_eq!(
        ids(&set),
        vec![0, 1],
        "one change-set for the whole outer transaction"
    );
}

#[test]
fn repeated_panics_never_leave_the_transaction_state_stuck() {
    let t = two();
    for i in 0..20 {
        let r = catch_unwind(AssertUnwindSafe(|| {
            txn(|| {
                txn(|| panic!("round {i}"));
            });
        }));
        assert!(r.is_err());
    }
    t.rig.run(|| t.a.set(1));
    assert_eq!(t.rig.one_set().entries.len(), 1);
}

#[test]
fn a_panicking_sink_does_not_lose_the_other_stores_change_sets() {
    let one = two();
    let other = Rig::new();
    other.cell.set_handle(0x0000_0002_0000_0002);
    let c = Signal::new(0_u32);
    other.cell.attach(&c, 0).unwrap();
    other.observe_all();

    let calls = Arc::new(AtomicUsize::new(0));
    let calls2 = calls.clone();
    let probe = Probe::new(move |_| {
        if calls2.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("sink failure");
        }
    });
    let result = catch_unwind(AssertUnwindSafe(|| {
        with_sink(probe.clone(), || {
            txn(|| {
                one.a.set(1);
                c.set(2);
            });
        });
    }));
    assert!(
        result.is_err(),
        "the sink's panic is re-raised after the commit"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the second store was still delivered"
    );
    assert_eq!(probe.capture.take_decoded().len(), 2);
    // Everything is consistent afterwards.
    let sink = CaptureSink::new();
    with_sink(sink.clone(), || one.a.set(5));
    assert_eq!(sink.take_decoded().len(), 1);
}

/// A value whose encoding can be made to panic.
#[derive(Clone)]
struct Bomb {
    armed: Arc<AtomicBool>,
    value: u32,
}

impl Encode for Bomb {
    fn encode(&self, w: &mut Writer) {
        assert!(!self.armed.load(Ordering::SeqCst), "encoder failure");
        self.value.encode(w);
    }
}

#[test]
fn a_panicking_encoder_is_contained_to_its_store() {
    let armed = Arc::new(AtomicBool::new(false));
    let bad = Rig::new();
    let bomb = Signal::new(Bomb {
        armed: armed.clone(),
        value: 0,
    });
    bad.cell.attach(&bomb, 0).unwrap();
    bad.observe_all();

    let good = Rig::new();
    good.cell.set_handle(0x0000_0009_0000_0001);
    let plain = Signal::new(0_u32);
    good.cell.attach(&plain, 0).unwrap();
    good.observe_all();

    armed.store(true, Ordering::SeqCst);
    let shared = CaptureSink::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        with_sink(shared.clone(), || {
            txn(|| {
                bomb.set(Bomb {
                    armed: armed.clone(),
                    value: 1,
                });
                plain.set(7);
            });
        });
    }));
    assert!(result.is_err());
    let sets = shared.take_decoded();
    assert_eq!(sets.len(), 1, "no half-built change-set escapes");
    assert_eq!(sets[0].entries[0].handle.0, 0x0000_0009_0000_0001);

    // Once the encoder behaves, the store works again.
    armed.store(false, Ordering::SeqCst);
    with_sink(shared.clone(), || {
        bomb.set(Bomb {
            armed: armed.clone(),
            value: 2,
        });
    });
    let sets = shared.take_decoded();
    assert_eq!(sets.len(), 1);
    assert_eq!(value_of::<u32>(&sets[0].entries[0]), 2);
}

#[test]
fn a_panicking_computed_at_commit_is_contained_and_recovers() {
    let rig = Rig::new();
    let src = Signal::new(1_u32);
    let inverse = Computed::new(&src, |v: &u32| {
        assert!(*v != 0, "division by zero");
        100 / v
    });
    rig.cell.attach(&src, 0).unwrap();
    rig.cell.attach_computed(&inverse, 1).unwrap();
    rig.observe_all();

    let result = catch_unwind(AssertUnwindSafe(|| rig.run(|| src.set(0))));
    assert!(result.is_err());
    assert!(
        rig.sets().is_empty(),
        "the store's change-set was abandoned, not half-sent"
    );

    rig.run(|| src.set(4));
    let set = rig.one_set();
    assert_eq!(
        value_of::<u32>(entry(&set, 1)),
        25,
        "the computed recovered"
    );
}

#[test]
fn an_observe_after_a_failed_commit_resynchronises_the_host() {
    let rig = Rig::new();
    let src = Signal::new(1_u32);
    let inverse = Computed::new(&src, |v: &u32| {
        assert!(*v != 0, "division by zero");
        100 / v
    });
    rig.cell.attach(&src, 0).unwrap();
    rig.cell.attach_computed(&inverse, 1).unwrap();
    rig.observe_all();
    let _ = catch_unwind(AssertUnwindSafe(|| rig.run(|| src.set(0))));
    // The abandoned change-set is exactly the case re-observing exists for.
    src.set(3);
    let entries = rig.observe_on(0);
    assert_eq!(value_of::<u32>(&entries[0]), 3);
}

#[test]
fn store_cells_can_be_shared_with_the_sink() {
    // A sink holding an `Arc<StoreCell>` (as the runtime does) must not create a cycle that
    // breaks anything: dropping everything at the end must simply work.
    let cell = StoreCell::new(1);
    let s = Signal::new(0_u32);
    cell.attach(&s, 0).unwrap();
    cell.set_handle(1);
    cell.observe(0, true, &mut Writer::new());
    let held = cell.clone();
    let probe = Probe::new(move |_| {
        let _ = held.signal_count();
    });
    with_sink(probe.clone(), || s.set(1));
    assert_eq!(probe.capture.len(), 1);
}
