//! `StoreCell::observe_and_deliver`: the observe path that builds a store's entries and hands
//! them to the host under the store's delivery lock (undra-runtime review 2026-09-30, finding M1,
//! and ADR-023). Tests named `rt_m1_` close that finding at this layer.

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use common::*;
use undra_signals::{ALL_SIGNALS, ChangeSink, Computed, Signal, txn, with_sink};
use undra_wire::payload::ChangeSet;
use undra_wire::{Decode, Encode, Reader, Writer};
use parking_lot::{Condvar, Mutex};

fn decode_set(payload: &[u8]) -> ChangeSet {
    let mut r = Reader::new(payload);
    let set = ChangeSet::decode(&mut r).expect("a complete change-set");
    r.finish().expect("no trailing bytes");
    set
}

/// A host mirror of one `u32` per signal id, fed in arrival order across threads.
#[derive(Default)]
struct Mirror {
    sets: Mutex<Vec<ChangeSet>>,
}

impl Mirror {
    fn push(&self, payload: &[u8]) {
        self.sets.lock().push(decode_set(payload));
    }

    fn sets(&self) -> Vec<ChangeSet> {
        self.sets.lock().clone()
    }

    /// The last value delivered for `signal_id`, applying the change-sets in arrival order.
    fn last(&self, signal_id: u32) -> Option<u32> {
        let mut last = None;
        for set in self.sets.lock().iter() {
            for e in &set.entries {
                if e.signal_id == signal_id {
                    last = Some(value_of::<u32>(e));
                }
            }
        }
        last
    }
}

impl ChangeSink for Mirror {
    fn deliver(&self, change_set: &[u8]) {
        self.push(change_set);
    }
}

#[test]
fn observe_and_deliver_hands_over_one_complete_change_set() {
    let rig = Rig::new();
    let a = Signal::new(7_u32);
    let b = Signal::new(9_u32);
    rig.cell.attach(&a, 0).unwrap();
    rig.cell.attach(&b, 1).unwrap();

    let before = undra_signals::next_txn_id();
    let mut received = Vec::new();
    let count = rig
        .cell
        .observe_and_deliver(&[ALL_SIGNALS], |payload| received.push(payload.to_vec()));
    assert_eq!(count, 2);
    assert_eq!(
        received.len(),
        1,
        "one change-set, delivered before the call returns"
    );
    let set = decode_set(&received[0]);
    assert!(set.txn_id > before, "a fresh transaction id");
    assert_eq!(ids(&set), [0, 1]);
    assert_eq!(set.entries[0].handle, handle());
    assert_eq!(value_of::<u32>(&set.entries[0]), 7);
    assert_eq!(value_of::<u32>(&set.entries[1]), 9);
    assert!(rig.cell.is_observed(0) && rig.cell.is_observed(1));

    // From now on commits are delivered, under ids above the observe's.
    rig.run(|| a.set(8));
    let next = rig.one_set();
    assert!(next.txn_id > set.txn_id);
    assert_eq!(value_of::<u32>(entry(&next, 0)), 8);
}

#[test]
fn observe_and_deliver_orders_and_deduplicates_ids_and_skips_unknown_ones() {
    let rig = Rig::new();
    let signals: Vec<Signal<u32>> = (0..3).map(Signal::new).collect();
    for (id, s) in signals.iter().enumerate() {
        rig.cell.attach(s, id as u32).unwrap();
    }
    let mut received = Vec::new();
    let count = rig
        .cell
        .observe_and_deliver(&[2, 0, 2, 99], |p| received.push(p.to_vec()));
    assert_eq!(count, 2);
    assert_eq!(ids(&decode_set(&received[0])), [0, 2]);
    assert!(!rig.cell.is_observed(1));

    // Nothing to observe: no change-set at all, and the caller's closure never runs.
    let called = AtomicBool::new(false);
    let none = rig.cell.observe_and_deliver(&[99, u32::MAX - 1], |_| {
        called.store(true, Ordering::SeqCst)
    });
    assert_eq!(none, 0);
    assert!(!called.load(Ordering::SeqCst));
    assert_eq!(
        rig.cell
            .observe_and_deliver(&[], |_| called.store(true, Ordering::SeqCst)),
        0
    );
    assert!(!called.load(Ordering::SeqCst));
}

/// An encoder that parks the observing thread in the middle of building the entries.
#[derive(Clone)]
struct Gate {
    value: u32,
    hold: Arc<(Mutex<GateState>, Condvar)>,
}

#[derive(Default)]
struct GateState {
    armed: bool,
    entered: bool,
    open: bool,
}

impl Gate {
    fn new(value: u32) -> Gate {
        Gate {
            value,
            hold: Arc::new((Mutex::new(GateState::default()), Condvar::new())),
        }
    }

    fn arm(&self) {
        self.hold.0.lock().armed = true;
    }

    fn wait_entered(&self) {
        let mut state = self.hold.0.lock();
        while !state.entered {
            assert!(
                !self
                    .hold
                    .1
                    .wait_for(&mut state, Duration::from_secs(10))
                    .timed_out(),
                "the observe never reached the gate"
            );
        }
    }

    fn open(&self) {
        self.hold.0.lock().open = true;
        self.hold.1.notify_all();
    }
}

impl Encode for Gate {
    fn encode(&self, w: &mut Writer) {
        let mut state = self.hold.0.lock();
        if state.armed {
            state.armed = false;
            state.entered = true;
            self.hold.1.notify_all();
            while !state.open {
                assert!(
                    !self
                        .hold
                        .1
                        .wait_for(&mut state, Duration::from_secs(10))
                        .timed_out(),
                    "the gate was never opened"
                );
            }
        }
        w.write_u32(self.value);
    }
}

/// The review's T2: the observing thread encodes `x = 1`, is held up, another thread sets
/// `x = 2` and commits, then the observe goes on. The host must end on 2, and the change-sets
/// of the store must reach it with ascending transaction ids.
#[test]
fn rt_m1_a_commit_racing_an_observe_never_leaves_the_host_on_the_older_value() {
    let rig = Rig::new();
    let x = Signal::new(1_u32);
    let gate = Signal::new(Gate::new(7));
    rig.cell.attach(&x, 0).unwrap();
    rig.cell.attach(&gate, 1).unwrap();
    let mirror = Arc::new(Mirror::default());
    let probe = gate.get();
    probe.arm();

    let observer = {
        let (cell, mirror) = (rig.cell.clone(), mirror.clone());
        std::thread::spawn(move || {
            cell.observe_and_deliver(&[ALL_SIGNALS], |payload| mirror.push(payload))
        })
    };
    probe.wait_entered(); // `x` is encoded (as 1); the gate's encoder is holding the observe

    // Another thread writes x = 2 and commits its transaction into the same mirror.
    let writer = {
        let (x, mirror) = (x.clone(), mirror.clone());
        std::thread::spawn(move || with_sink(mirror, || x.set(2)))
    };
    // The writer has written the value, and now either delivers straight away (the bug: its
    // change-set overtakes the observe's older entries) or waits for the delivery lock.
    while x.get() != 2 {
        std::thread::yield_now();
    }
    std::thread::sleep(Duration::from_millis(150));
    probe.open();
    observer.join().unwrap();
    writer.join().unwrap();

    assert_eq!(x.get(), 2);
    assert_eq!(
        mirror.last(0),
        Some(2),
        "the host mirror must end on the core's value: {:?}",
        mirror.sets()
    );
    let txns: Vec<u64> = mirror.sets().iter().map(|s| s.txn_id).collect();
    assert!(
        txns.windows(2).all(|w| w[0] < w[1]),
        "one store's change-sets arrive with ascending transaction ids: {txns:?}"
    );
}

/// While `deliver` runs, the store's delivery lock is held: a commit from another thread
/// cannot reach the sink until `deliver` has returned.
#[test]
fn rt_m1_the_delivery_lock_is_held_while_deliver_runs() {
    let rig = Rig::new();
    let x = Signal::new(1_u32);
    rig.cell.attach(&x, 0).unwrap();
    let mirror = Arc::new(Mirror::default());
    let delivered_by_writer_early = Arc::new(AtomicBool::new(false));

    let cell = rig.cell.clone();
    let mirror2 = mirror.clone();
    let early = delivered_by_writer_early.clone();
    let x2 = x.clone();
    cell.observe_and_deliver(&[0], |payload| {
        // Another thread writes and commits while the observe's change-set is being handed over.
        let writer = {
            let (x, mirror) = (x2.clone(), mirror2.clone());
            std::thread::spawn(move || with_sink(mirror, || x.set(5)))
        };
        std::thread::sleep(Duration::from_millis(150));
        // Nothing from the writer has arrived: it is waiting for the delivery lock.
        if !mirror2.sets().is_empty() {
            early.store(true, Ordering::SeqCst);
        }
        mirror2.push(payload);
        drop(writer); // detached: it finishes once the lock is released, below
    });
    assert!(
        !delivered_by_writer_early.load(Ordering::SeqCst),
        "a commit overtook the observe's delivery"
    );
    // Once the observe has finished, the writer's change-set follows it.
    for _ in 0..200 {
        if mirror.sets().len() == 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let sets = mirror.sets();
    assert_eq!(sets.len(), 2, "{sets:?}");
    assert!(sets[0].txn_id < sets[1].txn_id);
    assert_eq!(value_of::<u32>(entry(&sets[0], 0)), 1);
    assert_eq!(value_of::<u32>(entry(&sets[1], 0)), 5);
}

/// The observe records its transaction id, so a commit that holds a shared id the observe has
/// overtaken is given a fresh one instead of delivering an older id after a newer one.
#[test]
fn rt_m1_a_later_commit_never_reuses_an_id_the_observe_overtook() {
    const S2: u64 = 0x0000_0002_0000_0002;
    let s1 = Rig::new();
    let s2 = Rig::new();
    s2.cell.set_handle(S2);
    let a1 = Signal::new(0_u32);
    let a2 = Signal::new(0_u32);
    let b2 = Signal::new(0_u32);
    s1.cell.attach(&a1, 0).unwrap();
    s2.cell.attach(&a2, 0).unwrap();
    s2.cell.attach(&b2, 1).unwrap();
    s1.observe_all();
    s2.observe_all();

    // A sink for S1's commit that, while the transaction's shared id is already allocated,
    // runs an observe-and-deliver of S2 (which allocates a newer id and records it).
    struct ObserveS2 {
        s2: Arc<undra_signals::StoreCell>,
        seen: Mutex<Vec<(u64, u64)>>,
        done: AtomicBool,
    }
    impl ChangeSink for ObserveS2 {
        fn deliver(&self, change_set: &[u8]) {
            let set = decode_set(change_set);
            self.seen.lock().push((set.entries[0].handle.0, set.txn_id));
            if !self.done.swap(true, Ordering::SeqCst) {
                let seen = &self.seen;
                self.s2
                    .observe_and_deliver(&[0], |p| seen.lock().push((S2, decode_set(p).txn_id)));
            }
        }
    }
    let sink = Arc::new(ObserveS2 {
        s2: s2.cell.clone(),
        seen: Mutex::new(Vec::new()),
        done: AtomicBool::new(false),
    });
    with_sink(sink.clone(), || {
        txn(|| {
            a1.set(1);
            b2.set(1); // slot 1 of S2: the observe (slot 0) leaves it to the commit
        });
    });
    let s2_ids: Vec<u64> = sink
        .seen
        .lock()
        .iter()
        .filter(|(h, _)| *h == S2)
        .map(|(_, t)| *t)
        .collect();
    assert_eq!(
        s2_ids.len(),
        2,
        "the observe's change-set and the commit's: {:?}",
        sink.seen.lock()
    );
    assert!(
        s2_ids[0] < s2_ids[1],
        "S2 saw transaction ids {s2_ids:?}: they must ascend in delivery order"
    );
}

/// A computed that writes its own input: `observe_and_deliver` settles it within its passes
/// and commits the rest once the delivery lock is gone, so the host converges (the shape of
/// the review's L9, at the signals layer).
#[test]
fn rt_l9_leftover_writes_commit_after_the_entries_and_the_host_converges() {
    let rig = Rig::new();
    let n = Signal::new(0_i32);
    let writer = n.clone();
    let chase = Computed::new(&n, move |v: &i32| {
        if *v < 50 {
            writer.set(*v + 1);
        }
        *v
    });
    rig.cell.attach(&n, 0).unwrap();
    rig.cell.attach_computed(&chase, 1).unwrap();
    let mirror = Arc::new(Mirror::default());
    let mirror2 = mirror.clone();
    // The caller's transaction (what the runtime opens) keeps the leftovers for the end.
    with_sink(mirror.clone(), || {
        txn(|| {
            rig.cell
                .observe_and_deliver(&[ALL_SIGNALS], |p| mirror2.push(p));
        });
    });
    assert_eq!(n.get(), 50);
    let sets = mirror.sets();
    let txns: Vec<u64> = sets.iter().map(|s| s.txn_id).collect();
    assert!(txns.windows(2).all(|w| w[0] < w[1]), "{txns:?}");
    let last_i32 = |id: u32| {
        sets.iter()
            .flat_map(|s| s.entries.iter())
            .filter(|e| e.signal_id == id)
            .map(|e| i32::decode_exact(&e.value).unwrap())
            .next_back()
    };
    assert_eq!(
        last_i32(0),
        Some(50),
        "the host ends on the core's value: {sets:?}"
    );
}

/// A panic while delivering (or encoding) leaves nothing observed that the host never got.
#[test]
fn a_panicking_deliver_rolls_the_observe_back() {
    let rig = Rig::new();
    let a = Signal::new(1_u32);
    rig.cell.attach(&a, 0).unwrap();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        rig.cell
            .observe_and_deliver(&[0], |_| panic!("the host blew up"));
    }));
    assert!(outcome.is_err());
    assert!(
        !rig.cell.is_observed(0),
        "the host never received the value"
    );

    // The store works normally afterwards.
    let mut received = Vec::new();
    rig.cell
        .observe_and_deliver(&[0], |p| received.push(p.to_vec()));
    assert_eq!(received.len(), 1);
    assert!(rig.cell.is_observed(0));
}

#[test]
fn a_panicking_deliver_on_an_observed_slot_resends_its_value_with_the_next_commit() {
    let rig = Rig::new();
    let a = Signal::new(1_u32);
    let b = Signal::new(1_u32);
    rig.cell.attach(&a, 0).unwrap();
    rig.cell.attach(&b, 1).unwrap();
    rig.observe_all();
    let delivered = AtomicU32::new(0);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        rig.cell.observe_and_deliver(&[0], |_| {
            delivered.fetch_add(1, Ordering::SeqCst);
            panic!("the host blew up");
        });
    }));
    assert!(outcome.is_err());
    assert!(
        rig.cell.is_observed(0),
        "it was observed before and stays so"
    );
    // A commit of the *other* slot carries slot 0's current value too (marked unsent).
    rig.run(|| b.set(2));
    let set = rig.one_set();
    assert_eq!(ids(&set), [0, 1]);
}
