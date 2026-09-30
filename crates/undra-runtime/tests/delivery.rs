//! What the host ends up with after `observe` and `restore` (ADR-023; review findings M1 and L9).
//!
//! `observe` and restore phase 3 build a store's entries and hand them to the host under the
//! store's delivery lock, inside a transaction, through one shared path. The tests run a real
//! race (M1) and the review's self-writing computed (L9) and check the host's mirror against the
//! core.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::*;
use undra_meta::ids;
use undra_runtime::testing::{TestRuntime, unchecked_writes};
use undra_runtime::{Ctx, UndraObject, StoreObject, StoreRestorer};
use undra_signals::{ALL_SIGNALS, Computed, Signal, StoreCell};
use undra_wire::payload::ChangeSet;
use undra_wire::{Decode, Encode, Reader, WireError, Writer};
use parking_lot::{Condvar, Mutex};

// ----- M1: an observe racing a commit ---------------------------------------------------------

/// An encoder that parks the observing thread while it builds the entries.
#[derive(Clone)]
struct Gate {
    hold: Arc<(Mutex<GateState>, Condvar)>,
}

#[derive(Default)]
struct GateState {
    armed: bool,
    entered: bool,
    open: bool,
}

impl Gate {
    fn new() -> Gate {
        Gate {
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
        w.write_u32(7);
    }
}

struct Pair {
    cell: Arc<StoreCell>,
    x: Signal<i32>,
    g: Signal<Gate>,
}

impl UndraObject for Pair {
    const TYPE_ID: u32 = ids::type_id("DeliveryPair");
    const NAME: &'static str = "DeliveryPair";
}

impl StoreObject for Pair {
    fn cell(&self) -> &Arc<StoreCell> {
        &self.cell
    }

    fn restore(_: Ctx, _: &mut Reader<'_>) -> Result<Self, WireError> {
        unreachable!("not snapshotted in these tests")
    }
}

fn decoded_x(sets: &[ChangeSet]) -> Vec<(u64, i32)> {
    sets.iter()
        .flat_map(|cs| {
            cs.entries
                .iter()
                .filter(|e| e.signal_id == 0)
                .map(|e| (cs.txn_id, i32::decode_exact(&e.value).unwrap()))
        })
        .collect()
}

/// The review's T2: the observe has encoded `x = 1` and is held up building the next entry; another
/// thread sets `x = 2` and commits; the observe goes on. The host must end on the core's value,
/// and this store's change-sets must reach it with ascending transaction ids.
#[test]
fn m1_a_commit_racing_an_observe_never_leaves_the_host_on_the_older_value() {
    let t = TestRuntime::new();
    let rt = t.runtime().clone();
    let cell = StoreCell::new(Pair::TYPE_ID);
    let pair = Arc::new(Pair {
        x: Signal::new(1),
        g: Signal::new(Gate::new()),
        cell,
    });
    pair.cell.attach(&pair.x, 0).unwrap();
    pair.cell.attach(&pair.g, 1).unwrap();
    let h = rt.insert_store(pair.clone());
    let gate = pair.g.get();
    gate.arm();

    // The host observes the whole store, on its own thread.
    let observer = {
        let rt = rt.clone();
        std::thread::spawn(move || rt.observe(h.0, ALL_SIGNALS, true))
    };
    gate.wait_entered();

    // An embedder thread (release-build behaviour: the write checker would refuse it in a debug
    // build) writes x = 2 and commits while the observe is in the middle of its entries.
    let writer = {
        let (ctx, pair) = (t.ctx(), pair.clone());
        std::thread::spawn(move || {
            let _scope = ctx.enter();
            unchecked_writes(|| pair.x.set(2));
        })
    };
    while pair.x.get() != 2 {
        std::thread::yield_now();
    }
    // The writer either delivers straight away (the bug: its change-set overtakes the observe's
    // older entries) or waits for the delivery lock.
    std::thread::sleep(Duration::from_millis(150));
    gate.open();
    observer.join().unwrap();
    writer.join().unwrap();

    assert_eq!(pair.x.get(), 2);
    let sets = t.host().take_decoded_change_sets();
    let x = decoded_x(&sets);
    assert_eq!(
        x.last().map(|(_, v)| *v),
        Some(2),
        "the host mirror must end on the core's value: {sets:?}"
    );
    let txns: Vec<u64> = sets.iter().map(|s| s.txn_id).collect();
    assert!(
        txns.windows(2).all(|w| w[0] < w[1]),
        "one store's change-sets arrive with ascending transaction ids: {txns:?}"
    );
}

// ----- L9: a store whose computed keeps writing its own input ---------------------------------

/// `n` (0) and a computed (1) that writes `n + 1` whenever it evaluates below 50: it cannot
/// settle within the cell's passes, so the rest must reach the host as follow-up change-sets.
struct Chaser {
    cell: Arc<StoreCell>,
    n: Signal<i32>,
    _chase: Computed<i32>,
}

impl UndraObject for Chaser {
    const TYPE_ID: u32 = ids::type_id("DeliveryChaser");
    const NAME: &'static str = "DeliveryChaser";
}

impl Chaser {
    fn build(start: i32) -> Chaser {
        let cell = StoreCell::new(Self::TYPE_ID);
        let n = Signal::new(start);
        let writer = n.clone();
        let chase = Computed::new((&n,), move |(v,): (&i32,)| {
            let v = *v;
            if v < 50 {
                writer.set(v + 1);
            }
            v
        });
        cell.attach(&n, 0).unwrap();
        cell.attach_computed(&chase, 1).unwrap();
        Chaser {
            cell,
            n,
            _chase: chase,
        }
    }
}

impl StoreObject for Chaser {
    fn cell(&self) -> &Arc<StoreCell> {
        &self.cell
    }

    fn restore(_ctx: Ctx, r: &mut Reader<'_>) -> Result<Self, WireError> {
        let count = r.read_u32()?;
        let mut start = 0;
        for _ in 0..count {
            let id = r.read_u32()?;
            let value = r.read_bytes()?;
            if id == 0 {
                start = i32::decode_exact(value)?;
            }
        }
        Ok(Chaser::build(start))
    }
}

undra_meta::inventory::submit! {
    StoreRestorer {
        type_id: ids::type_id("DeliveryChaser"),
        restore: |ctx, handle, r| {
            let restored = <Chaser as StoreObject>::restore(ctx, r)?;
            restored.cell().set_handle(handle);
            Ok(Arc::new(restored))
        },
        cell: |any| any.downcast_ref::<Chaser>().map(<Chaser as StoreObject>::cell),
    }
}

fn last_n(sets: &[ChangeSet]) -> Option<i32> {
    decoded_x(sets).last().map(|(_, v)| *v)
}

/// The review's T12: `Runtime::observe` converged (the `54b3d14` fix), but restore phase 3 did
/// not wrap its re-observe in a transaction: the leftovers committed inside the cell's observe,
/// before the combined delivery, and the host ended at 7 while the core was at 50.
#[test]
fn l9_restore_phase_three_converges_on_the_settled_value_like_observe() {
    let t = TestRuntime::new();
    let rt = t.runtime().clone();
    let store = Arc::new(Chaser::build(0));
    let h = rt.insert_store(store.clone());
    let snapshot = rt.snapshot(); // n = 0, never evaluated

    rt.observe(h.0, ALL_SIGNALS, true);
    t.run_pending();
    let after_observe = t.host().take_decoded_change_sets();
    assert_eq!(
        last_n(&after_observe),
        Some(store.n.get()),
        "observe converges"
    );
    assert_eq!(store.n.get(), 50);

    rt.restore(&snapshot).unwrap();
    t.run_pending();
    let restored = rt.object::<Chaser>(h.0).unwrap();
    let sets = t.host().take_decoded_change_sets();
    assert_eq!(restored.n.get(), 50, "the restored store settled");
    assert_eq!(
        last_n(&sets),
        Some(restored.n.get()),
        "after a restore the host must converge on the core's settled value too: {} change-sets",
        sets.len()
    );
    let txns: Vec<u64> = sets.iter().map(|s| s.txn_id).collect();
    assert!(
        txns.windows(2).all(|w| w[0] < w[1]),
        "ascending transaction ids: {txns:?}"
    );
}

/// Restore delivers one self-consistent change-set per re-observed store (each under its own
/// store's delivery lock), not one combined set.
#[test]
fn restore_emits_one_change_set_per_observed_store_each_with_its_own_values() {
    let t = TestRuntime::new();
    let a = new_counter(&t, 1, "a");
    let b = new_counter(&t, 2, "b");
    let c = new_counter(&t, 3, "c"); // never observed: emits nothing
    t.runtime().observe(a.0, ALL_SIGNALS, true);
    t.runtime().observe(b.0, COUNT_SIGNAL, true);
    let snapshot = t.runtime().snapshot();
    call_counter(&t, a, ADD, 10, &enc(&5_i32));
    t.host().take_change_sets();

    t.runtime().restore(&snapshot).unwrap();
    let sets = t.host().take_decoded_change_sets();
    assert_eq!(sets.len(), 2, "{sets:?}");
    let handles: Vec<Vec<u64>> = sets
        .iter()
        .map(|s| {
            let mut hs: Vec<u64> = s.entries.iter().map(|e| e.handle.0).collect();
            hs.dedup();
            hs
        })
        .collect();
    assert_eq!(
        handles,
        [vec![a.0], vec![b.0]],
        "in handle order, never mixed"
    );
    assert_eq!(sets[0].entries.len(), 2, "a: ALL_SIGNALS");
    assert_eq!(sets[1].entries.len(), 1, "b: only the count signal");
    assert!(sets[0].txn_id < sets[1].txn_id);
    assert_eq!(decode_body::<i32>(&call_counter(&t, c, GET, 11, &[])), 3);
    // What the host observed before is observed after, too: a write reaches it.
    t.host().take_change_sets();
    call_counter(&t, a, ADD, 12, &enc(&1_i32));
    assert_eq!(t.host().take_decoded_change_sets().len(), 1);
}

/// A panic while observing (an encoder failing) leaves nothing observed, is contained, and the next
/// observe works. (The runtime also records the observation only after it succeeded, so a later
/// restore does not re-observe something the host never subscribed to: review N5.)
#[test]
fn a_failed_observe_leaves_nothing_observed_and_the_next_one_works() {
    struct Bomb(Arc<AtomicBool>);
    impl Clone for Bomb {
        fn clone(&self) -> Bomb {
            Bomb(self.0.clone())
        }
    }
    impl Encode for Bomb {
        fn encode(&self, w: &mut Writer) {
            assert!(!self.0.load(Ordering::SeqCst), "encoder failure");
            w.write_u32(0);
        }
    }
    struct Fragile {
        cell: Arc<StoreCell>,
        _bomb: Signal<Bomb>,
    }
    impl UndraObject for Fragile {
        const TYPE_ID: u32 = ids::type_id("DeliveryFragile");
        const NAME: &'static str = "DeliveryFragile";
    }
    impl StoreObject for Fragile {
        fn cell(&self) -> &Arc<StoreCell> {
            &self.cell
        }
        fn restore(_: Ctx, _: &mut Reader<'_>) -> Result<Self, WireError> {
            unreachable!()
        }
    }
    let t = TestRuntime::new();
    let armed = Arc::new(AtomicBool::new(true));
    let cell = StoreCell::new(Fragile::TYPE_ID);
    let bomb = Signal::new(Bomb(armed.clone()));
    cell.attach(&bomb, 0).unwrap();
    let store = Arc::new(Fragile { cell, _bomb: bomb });
    let h = t.runtime().insert_store(store.clone());
    t.runtime().observe(h.0, 0, true); // panics inside the encoder, contained
    assert!(!store.cell.is_observed(0));
    t.host().take_logs();
    // A fresh observe works once the encoder does, and is delivered.
    armed.store(false, Ordering::SeqCst);
    t.runtime().observe(h.0, 0, true);
    assert_eq!(t.host().take_decoded_change_sets().len(), 1);
}
