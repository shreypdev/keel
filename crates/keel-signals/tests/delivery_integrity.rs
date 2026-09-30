//! Regression tests for the adversarial review of 2026-09-30 (`.10x/reviews/`): what the host
//! ends up with after a commit or an observe was abandoned, written during, or interleaved with
//! other commits. Each test is named after the finding it closes (`h1_`, `m1_`, `m3_`, ...).
//!
//! The host is modelled by [`Host`], which applies every delivered entry the way the platform
//! runtimes do and fails the test on a patch that does not apply.

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::*;
use keel_signals::{ALL_SIGNALS, ChangeSink, Computed, Signal, txn, with_sink};
use keel_wire::payload::{ChangeEntry, ChangeOp, ChangeSet};
use keel_wire::{Encode, Writer};
use parking_lot::Mutex;

/// What a host knows about a store: a keyed list at signal 0 and `u32` values elsewhere.
#[derive(Default)]
struct Host {
    list: Vec<Todo>,
    values: std::collections::BTreeMap<u32, u32>,
    ops: Vec<(u32, ChangeOp)>,
}

impl Host {
    fn apply_entry(&mut self, e: &ChangeEntry, list_id: Option<u32>) {
        self.ops.push((e.signal_id, e.op));
        if Some(e.signal_id) == list_id {
            match e.op {
                ChangeOp::Full => self.list = value_of(e),
                ChangeOp::KeyedPatch => patch_of::<Todo>(e)
                    .apply(&mut self.list)
                    .expect("a delivered patch must apply to what the host has"),
                other => panic!("unexpected op {other:?}"),
            }
        } else {
            assert_eq!(e.op, ChangeOp::Full);
            self.values.insert(e.signal_id, value_of::<u32>(e));
        }
    }

    fn apply(&mut self, set: &ChangeSet, list_id: Option<u32>) {
        for e in &set.entries {
            self.apply_entry(e, list_id);
        }
    }

    fn apply_all(&mut self, sets: &[ChangeSet], list_id: Option<u32>) {
        for set in sets {
            self.apply(set, list_id);
        }
    }

    fn apply_entries(&mut self, entries: &[ChangeEntry], list_id: Option<u32>) {
        for e in entries {
            self.apply_entry(e, list_id);
        }
    }
}

/// A computed over `src` that panics while `armed`.
fn bomb_computed<T: keel_signals::SignalValue + 'static>(
    src: &Signal<T>,
    armed: &Arc<AtomicBool>,
) -> Computed<u32> {
    let armed = armed.clone();
    Computed::new(src, move |_: &T| {
        assert!(!armed.load(Ordering::SeqCst), "computed failure");
        7
    })
}

// ---------------------------------------------------------------------------------------------
// H1: an abandoned commit or observe must not leave the host silently diverged
// ---------------------------------------------------------------------------------------------

#[test]
fn h1_panicked_commit_does_not_strand_a_keyed_list_divergence() {
    // The review's repro: keyed rows at id 0 and a computed at id 1 that panics once.
    let rig = Rig::new();
    let rows = Signal::new(todos(2));
    let armed = Arc::new(AtomicBool::new(false));
    let bomb = bomb_computed(&rows, &armed);
    rig.cell.attach_keyed(&rows, 0, todo_key).unwrap();
    rig.cell.attach_computed(&bomb, 1).unwrap();

    let mut host = Host::default();
    host.apply_entries(&rig.observe_all(), Some(0));

    armed.store(true, Ordering::SeqCst);
    let result = catch_unwind(AssertUnwindSafe(|| {
        rig.run(|| rows.update(|r| r[0].done = true));
    }));
    assert!(result.is_err(), "the computed's panic is re-raised");
    assert!(rig.sets().is_empty(), "nothing is half-sent");
    armed.store(false, Ordering::SeqCst);

    rig.run(|| rows.update(|r| r.push(todo(3, "t3", false))));
    host.apply_all(&rig.sets(), Some(0));
    assert_eq!(
        host.list,
        rows.get(),
        "the host converges on the next commit"
    );
    assert!(host.list[0].done, "including the change the panic ate");
    assert!(
        host.ops.contains(&(0, ChangeOp::Full)),
        "resynchronised with a full value, not a patch against a base the host never saw"
    );
}

#[test]
fn h1_panicked_commit_does_not_strand_plain_values() {
    // a (0), a panicking computed (1) and b (2): the review's second repro.
    let rig = Rig::new();
    let a = Signal::new(1_u32);
    let armed = Arc::new(AtomicBool::new(false));
    let c = bomb_computed(&a, &armed);
    let b = Signal::new(0_u32);
    rig.cell.attach(&a, 0).unwrap();
    rig.cell.attach_computed(&c, 1).unwrap();
    rig.cell.attach(&b, 2).unwrap();

    let mut host = Host::default();
    host.apply_entries(&rig.observe_all(), None);
    assert_eq!(host.values[&0], 1);

    armed.store(true, Ordering::SeqCst);
    assert!(catch_unwind(AssertUnwindSafe(|| rig.run(|| a.set(5)))).is_err());
    armed.store(false, Ordering::SeqCst);

    for n in 1..=3 {
        rig.run(|| b.set(n));
        host.apply_all(&rig.sets(), None);
    }
    assert_eq!(host.values[&2], 3);
    assert_eq!(host.values[&0], 5, "a's abandoned change reached the host");
    assert_eq!(host.values[&1], 7, "and so did the computed's");
    // The retry rides on one commit only: the next one carries just what changed.
    rig.run(|| b.set(9));
    assert_eq!(ids(&rig.one_set()), vec![2]);
}

#[test]
fn h1_a_write_to_an_abandoned_slot_still_triggers_a_commit() {
    // The abandoned slot must not stay marked dirty with nobody to deliver it: a later write
    // to it has to be recorded and committed like any other.
    let rig = Rig::new();
    let a = Signal::new(1_u32);
    let armed = Arc::new(AtomicBool::new(false));
    let c = bomb_computed(&a, &armed);
    rig.cell.attach(&a, 0).unwrap();
    rig.cell.attach_computed(&c, 1).unwrap();
    rig.observe_all();

    armed.store(true, Ordering::SeqCst);
    assert!(catch_unwind(AssertUnwindSafe(|| rig.run(|| a.set(2)))).is_err());
    armed.store(false, Ordering::SeqCst);

    rig.run(|| a.set(3));
    let set = rig.one_set();
    assert_eq!(value_of::<u32>(entry(&set, 0)), 3);
    assert_eq!(value_of::<u32>(entry(&set, 1)), 7);
}

#[test]
fn h1_a_panicking_sink_makes_the_next_delivery_a_full_value() {
    // The sink may or may not have passed the patch on before it panicked; a full value is
    // safe either way, a patch against an uncertain base is not.
    let rig = Rig::new();
    let rows = Signal::new(todos(3));
    rig.cell.attach_keyed(&rows, 0, todo_key).unwrap();
    let mut host = Host::default();
    host.apply_entries(&rig.observe_all(), Some(0));

    struct Failing(AtomicBool);
    impl ChangeSink for Failing {
        fn deliver(&self, _change_set: &[u8]) {
            assert!(!self.0.load(Ordering::SeqCst), "sink failure");
        }
    }
    let failing = Arc::new(Failing(AtomicBool::new(true)));
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            with_sink(failing.clone(), || rows.update(|r| r[0].done = true));
        }))
        .is_err()
    );
    failing.0.store(false, Ordering::SeqCst);

    rig.run(|| rows.update(|r| r.push(todo(4, "t4", false))));
    host.apply_all(&rig.sets(), Some(0));
    assert_eq!(host.list, rows.get());
    assert_eq!(host.ops.last(), Some(&(0, ChangeOp::Full)));
}

#[test]
fn h1_a_failed_observe_leaves_nothing_observed_and_no_baseline() {
    // The review's third repro: a computed panics during `observe(ALL)`; the runtime throws the
    // entries away, so no slot may stay observed and no baseline may exist.
    let rig = Rig::new();
    let rows = Signal::new(todos(2));
    let armed = Arc::new(AtomicBool::new(true));
    let bomb = bomb_computed(&rows, &armed);
    rig.cell.attach_keyed(&rows, 0, todo_key).unwrap();
    rig.cell.attach_computed(&bomb, 1).unwrap();

    let mut out = Writer::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        rig.cell.observe(ALL_SIGNALS, true, &mut out)
    }));
    assert!(result.is_err());
    assert!(out.is_empty(), "no partial entries");
    assert!(!rig.cell.is_observed(0) && !rig.cell.is_observed(1));

    // Nothing is delivered for slots the host does not observe...
    rig.run(|| rows.update(|r| r.push(todo(3, "t3", false))));
    assert!(rig.sets().is_empty());

    // ...and once the host does observe, it starts from a full value.
    armed.store(false, Ordering::SeqCst);
    let mut host = Host::default();
    host.apply_entries(&rig.observe_all(), Some(0));
    assert_eq!(host.list, rows.get());
    rig.run(|| rows.update(|r| r[0].done = true));
    host.apply_all(&rig.sets(), Some(0));
    assert_eq!(host.list, rows.get());
}

#[test]
fn h1_a_failed_reobserve_of_an_observed_slot_does_not_swallow_its_pending_write() {
    let rig = Rig::new();
    let rows = Signal::new(todos(2));
    let armed = Arc::new(AtomicBool::new(false));
    let bomb = bomb_computed(&rows, &armed);
    rig.cell.attach_keyed(&rows, 0, todo_key).unwrap();
    rig.cell.attach_computed(&bomb, 1).unwrap();
    let mut host = Host::default();
    host.apply_entries(&rig.observe_on(0), Some(0));

    armed.store(true, Ordering::SeqCst);
    rig.run(|| {
        txn(|| {
            // A write that is waiting for its commit...
            rows.update(|r| r[0].done = true);
            // ...when a re-observe fails halfway (it clears the slot's dirty bit first).
            let result = catch_unwind(AssertUnwindSafe(|| {
                rig.cell.observe(ALL_SIGNALS, true, &mut Writer::new())
            }));
            assert!(result.is_err());
            assert!(rig.cell.is_observed(0), "slot 0 keeps its earlier state");
            assert!(!rig.cell.is_observed(1), "the computed was never observed");
        });
    });
    armed.store(false, Ordering::SeqCst);

    host.apply_all(&rig.sets(), Some(0));
    assert_eq!(
        host.list,
        rows.get(),
        "the pending write was still delivered"
    );
}

#[test]
fn h1_observing_again_after_an_abandoned_commit_clears_the_retry() {
    let rig = Rig::new();
    let a = Signal::new(1_u32);
    let armed = Arc::new(AtomicBool::new(false));
    let c = bomb_computed(&a, &armed);
    let b = Signal::new(0_u32);
    rig.cell.attach(&a, 0).unwrap();
    rig.cell.attach_computed(&c, 1).unwrap();
    rig.cell.attach(&b, 2).unwrap();
    rig.observe_all();

    armed.store(true, Ordering::SeqCst);
    assert!(catch_unwind(AssertUnwindSafe(|| rig.run(|| a.set(2)))).is_err());
    armed.store(false, Ordering::SeqCst);

    // The host resynchronises by observing everything: it now has the current values.
    let entries = rig.observe_all();
    assert_eq!(value_of::<u32>(&entries[0]), 2);
    rig.run(|| b.set(1));
    assert_eq!(
        ids(&rig.one_set()),
        vec![2],
        "nothing is sent twice after the resync"
    );
}

#[test]
fn h1_a_computed_that_keeps_panicking_holds_back_its_store_until_it_recovers() {
    // Documented trade-off: an abandoned change-set is retried by every later commit of the
    // store, so a computed that cannot be evaluated stalls the store loudly (each commit
    // re-raises its panic) instead of letting the host drift silently.
    let rig = Rig::new();
    let a = Signal::new(1_u32);
    let armed = Arc::new(AtomicBool::new(false));
    let c = bomb_computed(&a, &armed);
    let b = Signal::new(0_u32);
    rig.cell.attach(&a, 0).unwrap();
    rig.cell.attach_computed(&c, 1).unwrap();
    rig.cell.attach(&b, 2).unwrap();
    rig.observe_all();

    armed.store(true, Ordering::SeqCst);
    assert!(catch_unwind(AssertUnwindSafe(|| rig.run(|| a.set(2)))).is_err());
    assert!(catch_unwind(AssertUnwindSafe(|| rig.run(|| b.set(1)))).is_err());
    assert!(rig.sets().is_empty());

    armed.store(false, Ordering::SeqCst);
    rig.run(|| b.set(2));
    let set = rig.one_set();
    assert_eq!(ids(&set), vec![0, 1, 2]);
    assert_eq!(value_of::<u32>(entry(&set, 0)), 2);
    assert_eq!(value_of::<u32>(entry(&set, 2)), 2);
}

// ---------------------------------------------------------------------------------------------
// M1: writes made by computed closures during `observe` must not overtake the entries
// ---------------------------------------------------------------------------------------------

/// The review's setup: `hits` (0), `items` (1) and a computed (2) whose closure bumps `hits`.
struct Bumping {
    rig: Rig,
    hits: Signal<u32>,
    items: Signal<Vec<Todo>>,
    computed: Computed<u32>,
}

fn bumping() -> Bumping {
    let rig = Rig::new();
    let hits = Signal::new(0_u32);
    let items = Signal::new(todos(2));
    let bump = hits.clone();
    let computed = Computed::new(&items, move |items: &Vec<Todo>| {
        bump.update(|h| *h += 1);
        u32::try_from(items.len()).unwrap()
    });
    rig.cell.attach(&hits, 0).unwrap();
    rig.cell.attach_keyed(&items, 1, todo_key).unwrap();
    rig.cell.attach_computed(&computed, 2).unwrap();
    Bumping {
        rig,
        hits,
        items,
        computed,
    }
}

#[test]
fn m1_observe_entries_reflect_writes_made_by_computed_closures() {
    let b = bumping();
    let entries = b.rig.run(|| b.rig.observe_all());
    assert!(
        b.rig.sets().is_empty(),
        "no change-set may reach the sink ahead of the observe entries"
    );
    assert_eq!(b.hits.get(), 1, "the closure ran exactly once");
    let by_id = |id: u32| entries.iter().find(|e| e.signal_id == id).unwrap();
    assert_eq!(
        value_of::<u32>(by_id(0)),
        1,
        "the host receives the post-write value, not the value from before the closure ran"
    );
    assert_eq!(value_of::<u32>(by_id(2)), 2);
    assert_eq!(value_of::<Vec<Todo>>(by_id(1)), todos(2));

    // The write was absorbed: the host is in step, and the next commit is an ordinary one.
    b.rig
        .run(|| b.items.update(|l| l.push(todo(3, "t3", false))));
    let mut host = Host::default();
    host.apply_entries(&entries, Some(1));
    host.apply_all(&b.rig.sets(), Some(1));
    assert_eq!(host.list, b.items.get());
    assert_eq!(host.values[&0], b.hits.get());
    assert_eq!(host.values[&2], b.computed.get());
}

#[test]
fn m1_observe_without_closure_writes_encodes_every_target_once() {
    let encodes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rig = Rig::new();
    let probe = Signal::new(Probe::new(1, &encodes));
    rig.cell.attach(&probe, 0).unwrap();
    rig.observe_all();
    assert_eq!(
        count(&encodes),
        1,
        "the settle loop costs nothing when nothing writes"
    );
}

#[test]
fn m1_writes_to_slots_that_were_not_targeted_commit_after_the_entries_are_built() {
    let b = bumping();
    b.rig.observe_on(0);
    b.rig.sets();
    // Only the computed is (re)observed; its closure writes `hits`, an observed slot outside the
    // target set. That write is an ordinary commit, delivered once the entries have been built.
    let entries = b.rig.run(|| b.rig.observe_on(2));
    assert_eq!(entries.len(), 1);
    assert_eq!(value_of::<u32>(&entries[0]), 2);
    let set = b.rig.one_set();
    assert_eq!(ids(&set), vec![0]);
    assert_eq!(value_of::<u32>(entry(&set, 0)), 1);
}

#[test]
fn m1_a_keyed_list_written_during_observe_gets_a_matching_baseline() {
    let rig = Rig::new();
    let list = Signal::new(todos(2));
    let once = Arc::new(AtomicBool::new(true));
    let append = list.clone();
    let computed = Computed::new(&list, move |l: &Vec<Todo>| {
        if once.swap(false, Ordering::SeqCst) {
            append.update(|l| l.push(todo(3, "t3", false)));
        }
        u32::try_from(l.len()).unwrap()
    });
    rig.cell.attach_keyed(&list, 0, todo_key).unwrap();
    rig.cell.attach_computed(&computed, 1).unwrap();

    let mut host = Host::default();
    host.apply_entries(&rig.run(|| rig.observe_all()), Some(0));
    assert!(rig.sets().is_empty());
    assert_eq!(host.list, list.get());
    assert_eq!(host.list.len(), 3, "the closure's write is in the entries");

    rig.run(|| list.update(|l| l[0].done = true));
    host.apply_all(&rig.sets(), Some(0));
    assert_eq!(
        host.list,
        list.get(),
        "a patch against that baseline applies"
    );
}

#[test]
fn m1_a_closure_that_keeps_writing_its_own_input_cannot_loop_observe() {
    let rig = Rig::new();
    let n = Signal::new(0_u32);
    let bump = n.clone();
    let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = runs.clone();
    let computed = Computed::new(&n, move |v: &u32| {
        counted.fetch_add(1, Ordering::SeqCst);
        bump.set(*v + 1);
        *v
    });
    rig.cell.attach(&n, 0).unwrap();
    rig.cell.attach_computed(&computed, 1).unwrap();
    let entries = rig.run(|| rig.observe_all());
    assert_eq!(entries.len(), 2, "observe returns");
    // Eight settle passes, then the ordinary commit loop's own round cap.
    assert!(count(&runs) <= 8 + 1000 + 1, "ran {} times", count(&runs));
}

#[test]
fn m1_encode_signal_holds_closure_writes_until_the_value_is_produced() {
    /// A value that notes when it is encoded.
    #[derive(Clone)]
    struct Noted(Arc<AtomicBool>);
    impl Encode for Noted {
        fn encode(&self, w: &mut Writer) {
            self.0.store(true, Ordering::SeqCst);
            1_u32.encode(w);
        }
    }
    /// A sink that notes whether the value had been encoded when the change-set arrived.
    struct Ordered {
        encoded: Arc<AtomicBool>,
        seen_encoded: AtomicBool,
        delivered: AtomicBool,
    }
    impl ChangeSink for Ordered {
        fn deliver(&self, _change_set: &[u8]) {
            self.delivered.store(true, Ordering::SeqCst);
            self.seen_encoded
                .store(self.encoded.load(Ordering::SeqCst), Ordering::SeqCst);
        }
    }

    let rig = Rig::new();
    let hits = Signal::new(0_u32);
    let src = Signal::new(0_u32);
    let encoded = Arc::new(AtomicBool::new(false));
    let (bump, flag) = (hits.clone(), encoded.clone());
    let computed = Computed::new(&src, move |_: &u32| {
        bump.update(|h| *h += 1);
        Noted(flag.clone())
    });
    rig.cell.attach(&hits, 0).unwrap();
    rig.cell.attach(&src, 1).unwrap();
    rig.cell.attach_computed(&computed, 2).unwrap();
    rig.observe_on(0);

    let sink = Arc::new(Ordered {
        encoded,
        seen_encoded: AtomicBool::new(false),
        delivered: AtomicBool::new(false),
    });
    let mut out = Writer::new();
    with_sink(sink.clone(), || rig.cell.encode_signal(2, &mut out));
    assert!(
        sink.delivered.load(Ordering::SeqCst),
        "the write was committed"
    );
    assert!(
        sink.seen_encoded.load(Ordering::SeqCst),
        "and only after the value it was made for had been encoded"
    );
}

// ---------------------------------------------------------------------------------------------
// M2: commits of one store from several threads reach the sink in claim order
// ---------------------------------------------------------------------------------------------

/// A sink that holds up its first delivery until told to go on, and records deliveries in the
/// order they *complete*.
struct Gated {
    capture: Arc<keel_signals::testing::CaptureSink>,
    first: AtomicBool,
    entered: Mutex<std::sync::mpsc::Sender<()>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl ChangeSink for Gated {
    fn deliver(&self, change_set: &[u8]) {
        if self.first.swap(false, Ordering::SeqCst) {
            self.entered.lock().send(()).unwrap();
            self.release
                .lock()
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("released");
        }
        self.capture.deliver(change_set);
    }
}

type Latch = (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>);

fn gated() -> (Arc<Gated>, Latch) {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let sink = Arc::new(Gated {
        capture: keel_signals::testing::CaptureSink::new(),
        first: AtomicBool::new(true),
        entered: Mutex::new(entered_tx),
        release: Mutex::new(release_rx),
    });
    (sink, (entered_rx, release_tx))
}

#[test]
fn m2_concurrent_commits_of_one_store_reach_the_sink_in_claim_order() {
    // The review's repro: thread A removes item 2 (the diff is Remove{1} and the baseline
    // advances) and is held up before its delivery completes; thread B updates item 3 and
    // commits. B's patch is computed against A's baseline, so it must not reach the host first.
    let rig = Rig::new();
    let rows = Signal::new(todos(3));
    rig.cell.attach_keyed(&rows, 0, todo_key).unwrap();
    let mut host = Host::default();
    host.apply_entries(&rig.observe_all(), Some(0));

    let (sink, (entered, release)) = gated();
    let a = {
        let (rows, sink) = (rows.clone(), sink.clone());
        std::thread::spawn(move || {
            with_sink(sink, || rows.update(|l| drop(l.remove(1))));
        })
    };
    entered
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("A reached the sink");
    let b = {
        let (rows, sink) = (rows.clone(), sink.clone());
        std::thread::spawn(move || {
            with_sink(sink, || rows.update(|l| l[1].done = true));
        })
    };
    // Give B every chance to overtake A; it has to wait for A's delivery to finish.
    std::thread::sleep(std::time::Duration::from_millis(150));
    assert_eq!(
        sink.capture.len(),
        0,
        "B must not deliver while A's change-set is still being delivered"
    );
    release.send(()).unwrap();
    a.join().unwrap();
    b.join().unwrap();

    let sets = sink.capture.take_decoded();
    assert_eq!(sets.len(), 2);
    assert!(
        sets[0].txn_id < sets[1].txn_id,
        "ascending transaction ids: {} then {}",
        sets[0].txn_id,
        sets[1].txn_id
    );
    host.apply_all(&sets, Some(0));
    assert_eq!(
        host.list,
        rows.get(),
        "the host converges on the core's list"
    );
}

#[test]
fn m2_transaction_ids_seen_by_one_store_only_ever_grow() {
    // A transaction that commits stores S1 then S2 shares one id between them. If another
    // thread commits S2 in between, S2 must not see the shared (now older) id after that
    // thread's newer one.
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

    let (sink, (entered, release)) = gated();
    let a = {
        let (a1, a2, sink) = (a1.clone(), a2.clone(), sink.clone());
        std::thread::spawn(move || {
            with_sink(sink, || {
                txn(|| {
                    a1.set(1);
                    a2.set(1);
                });
            });
        })
    };
    // A's first delivery (S1) is held up, with the shared transaction id already allocated.
    entered
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("A reached the sink");
    // Meanwhile another thread commits a different slot of S2: it delivers straight away, under
    // a newer id.
    {
        let (b2, sink) = (b2.clone(), sink.clone());
        std::thread::spawn(move || with_sink(sink, || b2.set(2)))
            .join()
            .unwrap();
    }
    release.send(()).unwrap();
    a.join().unwrap();

    let sets = sink.capture.take_decoded();
    let s2_ids: Vec<u64> = sets
        .iter()
        .filter(|s| s.entries[0].handle.0 == S2)
        .map(|s| s.txn_id)
        .collect();
    assert_eq!(s2_ids.len(), 2, "{sets:?}");
    assert!(
        s2_ids[0] < s2_ids[1],
        "S2 saw transaction ids {s2_ids:?}: they must ascend in delivery order"
    );
}
