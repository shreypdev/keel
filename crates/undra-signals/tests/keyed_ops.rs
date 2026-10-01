//! Recorded list operations (ADR-027): `Signal<Vec<T>>::push`, `insert`, `remove`, `update_at`,
//! `move_item` and `clear` on a keyed list are sent as the ops they performed, O(ops), and what
//! the host replays from them always equals the core's list.
//!
//! The first half pins the wire: which ops each operation sends, what a raw write does to a
//! transaction that also recorded ops, what an abandoned commit, an unobserve and a re-observe do
//! to the log. The second half is a model-based property test: arbitrary interleavings of recorded
//! operations, raw writes, transactions, aborts (a computed that panics) and observe / unobserve
//! cycles, with a host mirror that applies every delivered change-set.

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::*;
use proptest::prelude::*;
use undra_signals::testing::CaptureSink;
use undra_signals::{ALL_SIGNALS, Computed, Signal, StoreCell, txn, with_sink};
use undra_wire::payload::{ChangeEntry, ChangeOp, ChangeSet};
use undra_wire::{KeyedPatch, PatchOp, Reader};

// ---------------------------------------------------------------------------------------------
// The wire: what each operation sends
// ---------------------------------------------------------------------------------------------

/// A store with `list` (0, keyed) and `tag` (1, plain), observed from the start.
struct Fixture {
    rig: Rig,
    list: Signal<Vec<Todo>>,
    tag: Signal<u32>,
}

fn fixture(initial: Vec<Todo>) -> Fixture {
    let rig = Rig::new();
    let list = Signal::new(initial);
    let tag = Signal::new(0_u32);
    rig.cell.attach_keyed(&list, 0, todo_key).unwrap();
    rig.cell.attach(&tag, 1).unwrap();
    rig.observe_on(0);
    Fixture { rig, list, tag }
}

impl Fixture {
    /// Runs `f` and returns the ops of the single patch it produced for the list.
    fn ops(&self, f: impl FnOnce()) -> Vec<PatchOp<Todo>> {
        self.rig.run(f);
        let set = self.rig.one_set();
        patch_of::<Todo>(entry(&set, 0)).ops
    }
}

fn insert(index: u32, item: Todo) -> PatchOp<Todo> {
    PatchOp::Insert { index, item }
}

#[test]
fn every_operation_sends_the_op_it_performed() {
    let f = fixture(todos(5));
    assert_eq!(
        f.ops(|| f.list.push(todo(6, "six", false))),
        vec![insert(5, todo(6, "six", false))]
    );
    assert_eq!(
        f.ops(|| f.list.insert(2, todo(7, "seven", true))),
        vec![insert(2, todo(7, "seven", true))]
    );
    assert_eq!(
        f.ops(|| assert_eq!(f.list.remove(0), todo(1, "t1", false))),
        vec![PatchOp::Remove { index: 0 }]
    );
    assert_eq!(
        f.ops(|| f.list.update_at(3, |t| t.done = true)),
        vec![PatchOp::Update {
            index: 3,
            item: todo(4, "t4", true)
        }],
        "an update carries the whole changed item (SPEC 3.8)"
    );
    assert_eq!(
        f.ops(|| f.list.move_item(1, 4)),
        vec![PatchOp::Move { from: 1, to: 4 }]
    );
    assert_eq!(f.ops(|| f.list.clear()), vec![PatchOp::Clear]);
}

#[test]
fn clearing_is_a_clear_op_not_a_full_value() {
    // The raw path would send the full value (no key overlap); the recorded one says so in one
    // byte.
    let f = fixture(todos(3));
    f.rig.run(|| f.list.clear());
    let set = f.rig.one_set();
    assert_eq!(entry(&set, 0).op, ChangeOp::KeyedPatch);
    assert_eq!(entry(&set, 0).value, vec![1, 0, 0, 0, 4]);
}

#[test]
fn a_recorded_patch_is_exactly_the_bytes_the_wire_defines() {
    // SPEC 3.8 layout, byte for byte: count, then tag and operands per op.
    let f = fixture(vec![todo(1, "a", false)]);
    f.rig.run(|| {
        txn(|| {
            f.list.insert(0, todo(2, "b", true));
            f.list.move_item(0, 1);
            f.list.remove(0);
        });
    });
    let set = f.rig.one_set();
    let value = &entry(&set, 0).value;
    let mut expected = undra_wire::Writer::new();
    KeyedPatch {
        ops: vec![
            insert(0, todo(2, "b", true)),
            PatchOp::Move { from: 0, to: 1 },
            PatchOp::Remove { index: 0 },
        ],
    }
    .encode(&mut expected);
    assert_eq!(value.as_slice(), expected.as_slice());
}

#[test]
fn a_transaction_of_recorded_operations_is_one_patch_of_its_ops_in_order() {
    let f = fixture(todos(4));
    let mut replica = f.list.get();
    let ops = f.ops(|| {
        txn(|| {
            f.list.push(todo(10, "a", false));
            f.list.insert(0, todo(11, "b", false));
            f.list.update_at(1, |t| t.title = "renamed".into());
            f.list.move_item(0, 3);
            f.list.remove(2);
        });
    });
    assert_eq!(ops.len(), 5, "one op per operation, no more");
    KeyedPatch { ops }.apply(&mut replica).unwrap();
    assert_eq!(replica, f.list.get());
}

#[test]
fn operations_that_change_nothing_send_an_empty_patch() {
    let f = fixture(todos(3));
    assert!(f.ops(|| f.list.move_item(1, 1)).is_empty());
    let empty = fixture(Vec::new());
    assert!(empty.ops(|| empty.list.clear()).is_empty());
}

#[test]
fn the_recorded_path_sends_what_was_recorded_where_the_diff_would_send_the_full_value() {
    // More than half removed: the raw path falls back to the full value (SPEC 3.8). Recorded
    // removals are sent as recorded.
    let raw = fixture(todos(10));
    raw.rig.run(|| raw.list.update(|l| l.truncate(3)));
    assert_eq!(entry(&raw.rig.one_set(), 0).op, ChangeOp::Full);

    let recorded = fixture(todos(10));
    let ops = recorded.ops(|| {
        txn(|| {
            for _ in 0..7 {
                recorded.list.remove(3);
            }
        });
    });
    assert_eq!(ops.len(), 7);
    let mut replica = todos(10);
    KeyedPatch { ops }.apply(&mut replica).unwrap();
    assert_eq!(replica, recorded.list.get());
}

#[test]
fn a_raw_write_makes_the_whole_transaction_a_diff() {
    // Recorded ops, then a raw update: the raw write invalidates the log, the commit diffs.
    // The diff of "everything removed, one new row" is the full value; the recorded log would
    // have been Clear + Insert.
    let f = fixture(todos(3));
    f.rig.run(|| {
        txn(|| {
            f.list.clear();
            f.list.update(|l| l.push(todo(9, "new", false)));
        });
    });
    let set = f.rig.one_set();
    let e = entry(&set, 0);
    assert_eq!(e.op, ChangeOp::Full);
    assert_eq!(value_of::<Vec<Todo>>(e), vec![todo(9, "new", false)]);
}

#[test]
fn recorded_operations_after_a_raw_write_do_not_resurrect_the_log() {
    let f = fixture(todos(3));
    f.rig.run(|| {
        txn(|| {
            f.list.update(|l| l.clear());
            f.list.push(todo(7, "p", false));
            f.list.push(todo(8, "q", false));
        });
    });
    let set = f.rig.one_set();
    let e = entry(&set, 0);
    // Diff of [1,2,3] -> [7,8]: no key overlap, so the full value; a log of two inserts on the
    // baseline of three items would have replayed to the wrong list.
    assert_eq!(e.op, ChangeOp::Full);
    assert_eq!(value_of::<Vec<Todo>>(e), f.list.get());
}

#[test]
fn set_and_replace_take_the_diff_path() {
    let f = fixture(todos(5));
    let mut replica = f.list.get();
    // Overlapping keys: a patch, not recorded ops and not the full value.
    let next: Vec<Todo> = (2..=6)
        .map(|id| todo(id, &format!("t{id}"), false))
        .collect();
    f.rig.run(|| f.list.replace(next.clone()));
    let set = f.rig.one_set();
    assert_eq!(entry(&set, 0).op, ChangeOp::KeyedPatch);
    patch_of::<Todo>(entry(&set, 0))
        .apply(&mut replica)
        .unwrap();
    assert_eq!(replica, next);
    // No overlap: the full value.
    f.rig.run(|| {
        f.list.set(
            todos(2)
                .into_iter()
                .map(|t| todo(t.id + 100, "x", false))
                .collect(),
        )
    });
    assert_eq!(entry(&f.rig.one_set(), 0).op, ChangeOp::Full);
}

#[test]
fn the_log_starts_empty_after_every_commit() {
    let f = fixture(todos(3));
    assert_eq!(f.ops(|| f.list.push(todo(4, "a", false))).len(), 1);
    assert_eq!(f.ops(|| f.list.push(todo(5, "b", false))).len(), 1);
    // A raw commit in between does not leave anything behind either.
    f.rig.run(|| f.list.update(|l| l[0].done = true));
    f.rig.sets();
    assert_eq!(f.ops(|| f.list.push(todo(6, "c", false))).len(), 1);
}

#[test]
fn recording_does_not_need_the_keys_to_be_unique() {
    // The host replays by position; the diff would have sent the full value.
    let f = fixture(todos(2));
    let ops = f.ops(|| f.list.push(todo(1, "dup", false)));
    assert_eq!(ops, vec![insert(2, todo(1, "dup", false))]);
}

#[test]
fn a_keyed_list_next_to_plain_signals_shares_the_change_set() {
    let f = fixture(todos(2));
    f.rig.observe_all();
    f.rig.sets();
    f.rig.run(|| {
        txn(|| {
            f.tag.set(5);
            f.list.push(todo(3, "x", false));
        });
    });
    let set = f.rig.one_set();
    assert_eq!(entry(&set, 0).op, ChangeOp::KeyedPatch);
    assert_eq!(value_of::<u32>(entry(&set, 1)), 5);
}

#[test]
fn a_list_attached_as_a_plain_signal_is_sent_in_full() {
    let rig = Rig::new();
    let list = Signal::new(todos(2));
    rig.cell.attach(&list, 0).unwrap();
    rig.observe_on(0);
    rig.run(|| list.push(todo(3, "x", false)));
    let set = rig.one_set();
    assert_eq!(entry(&set, 0).op, ChangeOp::Full);
    assert_eq!(value_of::<Vec<Todo>>(entry(&set, 0)), list.get());
}

// ---------------------------------------------------------------------------------------------
// Observation, abandonment and the log
// ---------------------------------------------------------------------------------------------

#[test]
fn an_unobserved_list_records_nothing_and_observing_sends_the_full_value() {
    let f = fixture(todos(3));
    f.rig.observe_off(0);
    f.rig.run(|| {
        f.list.push(todo(4, "a", false));
        f.list.remove(0);
    });
    assert!(f.rig.sets().is_empty());
    let entries = f.rig.observe_on(0);
    assert_eq!(entries[0].op, ChangeOp::Full);
    assert_eq!(value_of::<Vec<Todo>>(&entries[0]), f.list.get());
    // The operations made while unobserved are not replayed on top of the fresh baseline.
    let ops = f.ops(|| f.list.push(todo(5, "b", false)));
    assert_eq!(ops, vec![insert(3, todo(5, "b", false))]);
}

#[test]
fn re_observing_drops_what_was_recorded_before() {
    let f = fixture(todos(3));
    // Recorded but the host re-observes before any commit claims it: no commit happens
    // because the change-set is consumed by observe's own clearing of the dirty bit.
    let again = f.rig.run(|| {
        txn(|| {
            f.list.push(todo(4, "a", false));
            f.rig.observe_on(0)
        })
    });
    assert_eq!(value_of::<Vec<Todo>>(&again[0]), f.list.get());
    f.rig.sets();
    let ops = f.ops(|| f.list.push(todo(5, "b", false)));
    assert_eq!(ops, vec![insert(4, todo(5, "b", false))]);
}

#[test]
fn an_unobserved_no_coalesce_list_is_sent_in_full_and_records_nothing() {
    let rig = Rig::new();
    let list = Signal::new(todos(3));
    rig.cell.attach_keyed(&list, 0, todo_key).unwrap();
    rig.cell.set_no_coalesce(0).unwrap();
    rig.run(|| list.push(todo(4, "a", false)));
    let set = rig.one_set();
    assert_eq!(entry(&set, 0).op, ChangeOp::Full);
    assert_eq!(value_of::<Vec<Todo>>(entry(&set, 0)), list.get());
}

/// A list at 0 and, at 1, a plain slot whose encoder panics while armed (an abandoned
/// change-set, ADR-019 H1), with a closure that dirties it.
fn bombed(initial: Vec<Todo>) -> (Rig, Signal<Vec<Todo>>, Arc<AtomicBool>, impl Fn()) {
    let rig = Rig::new();
    let list = Signal::new(initial);
    let armed = Arc::new(AtomicBool::new(false));
    let bomb = Signal::new(EncodeBomb::new(&armed, 7));
    rig.cell.attach_keyed(&list, 0, todo_key).unwrap();
    rig.cell.attach(&bomb, 1).unwrap();
    rig.observe_all();
    let touch = move || bomb.update(|b| b.value = 7);
    (rig, list, armed, touch)
}

#[test]
fn an_abandoned_commit_drops_the_log_and_the_next_delivery_is_the_full_value() {
    let (rig, list, armed, touch) = bombed(todos(3));
    armed.store(true, Ordering::SeqCst);
    let aborted = catch_unwind(AssertUnwindSafe(|| {
        rig.run(|| {
            txn(|| {
                list.push(todo(4, "a", false));
                list.remove(0);
                touch();
            });
        });
    }));
    assert!(aborted.is_err());
    assert!(rig.sets().is_empty());
    armed.store(false, Ordering::SeqCst);

    // The ops the abandoned commit took are gone with the baseline: a replay of them on top of
    // the next delivery would apply them twice.
    rig.run(|| list.push(todo(5, "b", false)));
    let set = rig.one_set();
    let e = entry(&set, 0);
    assert_eq!(e.op, ChangeOp::Full);
    assert_eq!(value_of::<Vec<Todo>>(e), list.get());

    // And recording resumes from the fresh baseline.
    rig.run(|| list.push(todo(6, "c", false)));
    let set = rig.one_set();
    assert_eq!(
        patch_of::<Todo>(entry(&set, 0)).ops,
        vec![insert(4, todo(6, "c", false))]
    );
}

#[test]
fn writes_made_between_an_abandoned_commit_and_the_next_are_part_of_the_full_value() {
    let (rig, list, armed, touch) = bombed(todos(2));
    armed.store(true, Ordering::SeqCst);
    assert!(
        catch_unwind(AssertUnwindSafe(|| rig.run(|| txn(|| {
            list.push(todo(3, "a", false));
            touch();
        }))))
        .is_err()
    );
    armed.store(false, Ordering::SeqCst);
    // Not armed any more, but the slot is unsent: these are not recorded, the full value is sent.
    rig.run(|| {
        txn(|| {
            list.remove(1);
            list.push(todo(9, "z", false));
        });
    });
    let set = rig.one_set();
    assert_eq!(entry(&set, 0).op, ChangeOp::Full);
    assert_eq!(value_of::<Vec<Todo>>(entry(&set, 0)), list.get());
}

#[test]
fn operations_made_while_no_sink_is_installed_are_sent_with_the_next_commit() {
    // Without a sink a commit consumes the slot and delivers nothing; the log keeps the ops so
    // the next commit that has a sink still describes everything since the baseline.
    let f = fixture(todos(3));
    f.list.push(todo(4, "a", false));
    f.list.remove(0);
    assert!(f.rig.sets().is_empty());
    let mut replica = todos(3);
    let ops = f.ops(|| f.list.push(todo(5, "b", false)));
    assert_eq!(ops.len(), 3);
    KeyedPatch { ops }.apply(&mut replica).unwrap();
    assert_eq!(replica, f.list.get());
}

#[test]
fn a_computed_that_writes_the_list_during_observe_leaves_a_matching_baseline() {
    let rig = Rig::new();
    let list = Signal::new(todos(2));
    let once = Arc::new(AtomicBool::new(true));
    let writer = list.clone();
    let computed = Computed::new(&list, move |l: &Vec<Todo>| {
        if once.swap(false, Ordering::SeqCst) {
            writer.push(todo(3, "from the closure", false));
        }
        u32::try_from(l.len()).unwrap()
    });
    rig.cell.attach_keyed(&list, 0, todo_key).unwrap();
    rig.cell.attach_computed(&computed, 1).unwrap();

    let entries = rig.run(|| rig.observe_all());
    assert_eq!(value_of::<Vec<Todo>>(&entries[0]), list.get());
    assert!(
        rig.sets().is_empty(),
        "the write is absorbed into the entries"
    );
    let ops = {
        rig.run(|| list.push(todo(4, "later", false)));
        patch_of::<Todo>(entry(&rig.one_set(), 0)).ops
    };
    assert_eq!(ops, vec![insert(3, todo(4, "later", false))]);
}

#[test]
fn a_transaction_longer_than_the_log_is_still_replayed_correctly() {
    // More ops than the log keeps: it gives up and the commit diffs, which must agree.
    let f = fixture(todos(10));
    let mut replica = f.list.get();
    f.rig.run(|| {
        txn(|| {
            for n in 0..6_000_u32 {
                f.list.push(todo(1_000 + n, "bulk", false));
            }
        });
    });
    let set = f.rig.one_set();
    let e = entry(&set, 0);
    match e.op {
        ChangeOp::Full => replica = value_of(e),
        ChangeOp::KeyedPatch => patch_of::<Todo>(e).apply(&mut replica).unwrap(),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(replica, f.list.get());
    // And recording resumes.
    let ops = f.ops(|| f.list.push(todo(9_999_999, "after", false)));
    assert_eq!(ops.len(), 1);
}

#[test]
fn a_panicking_update_at_makes_the_next_commit_a_diff_that_carries_the_change() {
    let f = fixture(todos(3));
    let mut replica = f.list.get();
    f.rig.run(|| {
        let result = catch_unwind(AssertUnwindSafe(|| {
            f.list.update_at(1, |t| {
                t.title = "half done".into();
                panic!("boom");
            });
        }));
        assert!(result.is_err());
    });
    let set = f.rig.one_set();
    patch_of::<Todo>(entry(&set, 0))
        .apply(&mut replica)
        .unwrap();
    assert_eq!(replica, f.list.get());
    assert_eq!(replica[1].title, "half done");
}

#[test]
fn recorded_operations_reach_computeds_and_the_item_clone_is_paid_once() {
    use std::sync::atomic::AtomicUsize;
    #[derive(Debug)]
    struct Counted {
        id: u32,
        clones: Arc<AtomicUsize>,
    }
    impl Clone for Counted {
        fn clone(&self) -> Self {
            self.clones.fetch_add(1, Ordering::SeqCst);
            Counted {
                id: self.id,
                clones: Arc::clone(&self.clones),
            }
        }
    }
    impl undra_wire::Encode for Counted {
        fn encode(&self, w: &mut undra_wire::Writer) {
            self.id.encode(w);
        }
    }
    let clones = Arc::new(AtomicUsize::new(0));
    let rig = Rig::new();
    let make = |id| Counted {
        id,
        clones: Arc::clone(&clones),
    };
    let list = Signal::new((0..100).map(make).collect::<Vec<_>>());
    let count = Computed::new(&list, |l: &Vec<Counted>| u32::try_from(l.len()).unwrap());
    rig.cell
        .attach_keyed(&list, 0, |c: &Counted| u64::from(c.id))
        .unwrap();
    rig.cell.attach_computed(&count, 1).unwrap();
    rig.observe_all();
    let before = clones.load(Ordering::SeqCst);
    rig.run(|| list.push(make(1_000)));
    let set = rig.one_set();
    assert_eq!(
        value_of::<u32>(entry(&set, 1)),
        101,
        "the computed saw the push"
    );
    assert_eq!(
        clones.load(Ordering::SeqCst) - before,
        1,
        "one clone for the log; no list-wide clone, and the baseline takes the item by move"
    );
}

// ---------------------------------------------------------------------------------------------
// The model: arbitrary interleavings, a host mirror
// ---------------------------------------------------------------------------------------------

const LIST: u32 = 0;
const TICK: u32 = 1;
const BOMB: u32 = 2;

struct Core {
    cell: Arc<StoreCell>,
    list: Signal<Vec<Todo>>,
    tick: Signal<u32>,
    armed: Arc<AtomicBool>,
    sink: Arc<CaptureSink>,
}

fn core() -> Core {
    let cell = StoreCell::new(0xA11CE);
    cell.set_handle(HANDLE);
    let list = Signal::new(todos(3));
    let tick = Signal::new(0_u32);
    let armed = Arc::new(AtomicBool::new(false));
    let bomb = {
        let armed = Arc::clone(&armed);
        Computed::new(&list, move |l: &Vec<Todo>| {
            assert!(!armed.load(Ordering::SeqCst), "computed failure");
            u32::try_from(l.len()).unwrap()
        })
    };
    cell.attach_keyed(&list, LIST, todo_key).unwrap();
    cell.attach(&tick, TICK).unwrap();
    cell.attach_computed(&bomb, BOMB).unwrap();
    Core {
        cell,
        list,
        tick,
        armed,
        sink: CaptureSink::new(),
    }
}

/// What the host knows.
#[derive(Default)]
struct Mirror {
    observed: [bool; 3],
    list: Vec<Todo>,
    last_txn: u64,
}

impl Mirror {
    fn apply_entry(&mut self, e: &ChangeEntry) {
        match (e.signal_id, e.op) {
            (LIST, ChangeOp::Full) => self.list = value_of(e),
            (LIST, ChangeOp::KeyedPatch) => {
                let mut r = Reader::new(&e.value);
                let patch = KeyedPatch::<Todo>::decode(&mut r).unwrap();
                r.finish().unwrap();
                patch
                    .apply(&mut self.list)
                    .expect("a delivered patch always applies to what the host has");
            }
            (TICK | BOMB, ChangeOp::Full) => {}
            other => panic!("unexpected entry {other:?}"),
        }
    }

    fn apply_set(&mut self, set: &ChangeSet) {
        assert!(set.txn_id > self.last_txn, "commit order");
        self.last_txn = set.txn_id;
        let mut seen = [false; 3];
        for e in &set.entries {
            assert!(!seen[e.signal_id as usize], "one entry per signal");
            seen[e.signal_id as usize] = true;
            assert!(
                self.observed[e.signal_id as usize],
                "an unobserved signal was delivered"
            );
            self.apply_entry(e);
        }
    }
}

#[derive(Clone, Debug)]
enum Edit {
    // Recorded.
    Push(u32),
    Insert(usize, u32),
    Remove(usize),
    UpdateAt(usize),
    Move(usize, usize),
    Clear,
    // Raw.
    RawToggle(usize),
    RawPush(u32),
    RawRotate(usize),
    Set(Vec<u32>),
    Replace(Vec<u32>),
    // Not the list.
    Tick(u32),
}

/// What an edit did to the op log.
#[derive(PartialEq)]
enum Effect {
    /// One op was recorded (or none: the operation changed nothing).
    Recorded(usize),
    Raw,
    NotTheList,
}

impl Core {
    fn edit(&self, e: &Edit) -> Effect {
        let len = self.list.with(Vec::len);
        let recorded = Effect::Recorded;
        match e {
            Edit::Push(id) => {
                self.list.push(todo(*id, "p", false));
                recorded(1)
            }
            Edit::Insert(at, id) => {
                self.list.insert(at % (len + 1), todo(*id, "i", true));
                recorded(1)
            }
            Edit::Remove(at) if len > 0 => {
                self.list.remove(at % len);
                recorded(1)
            }
            Edit::UpdateAt(at) if len > 0 => {
                self.list.update_at(at % len, |t| {
                    t.done = !t.done;
                    t.title.push('u');
                });
                recorded(1)
            }
            Edit::Move(a, b) if len > 0 => {
                let (from, to) = (a % len, b % len);
                self.list.move_item(from, to);
                recorded(usize::from(from != to))
            }
            Edit::Clear => {
                self.list.clear();
                recorded(usize::from(len > 0))
            }
            // Nothing to act on in an empty list: no write at all.
            Edit::Remove(_) | Edit::UpdateAt(_) | Edit::Move(..) => Effect::NotTheList,
            Edit::RawToggle(at) => {
                if len > 0 {
                    self.list.update(|l| l[at % len].done = !l[at % len].done);
                } else {
                    self.list.update(|_| {});
                }
                Effect::Raw
            }
            Edit::RawPush(id) => {
                self.list.update(|l| l.push(todo(*id, "r", false)));
                Effect::Raw
            }
            Edit::RawRotate(by) => {
                self.list.update(|l| {
                    if !l.is_empty() {
                        let by = by % l.len();
                        l.rotate_left(by);
                    }
                });
                Effect::Raw
            }
            Edit::Set(ids) => {
                self.list
                    .set(ids.iter().map(|id| todo(*id, "s", false)).collect());
                Effect::Raw
            }
            Edit::Replace(ids) => {
                self.list
                    .replace(ids.iter().map(|id| todo(*id, "x", true)).collect());
                Effect::Raw
            }
            Edit::Tick(v) => {
                self.tick.set(*v);
                Effect::NotTheList
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Step {
    Edit(Edit),
    Txn(Vec<Edit>),
    /// A transaction under an armed computed: if the list is observed, the commit aborts.
    Abort(Vec<Edit>),
    ObserveOn(u32),
    ObserveOff(u32),
    ObserveAllOn,
    ObserveAllOff,
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        4 => (0_u32..12).prop_map(Edit::Push),
        3 => (0_usize..40, 0_u32..12).prop_map(|(a, id)| Edit::Insert(a, id)),
        3 => (0_usize..40).prop_map(Edit::Remove),
        3 => (0_usize..40).prop_map(Edit::UpdateAt),
        3 => (0_usize..40, 0_usize..40).prop_map(|(a, b)| Edit::Move(a, b)),
        1 => Just(Edit::Clear),
        2 => (0_usize..40).prop_map(Edit::RawToggle),
        1 => (0_u32..12).prop_map(Edit::RawPush),
        1 => (0_usize..40).prop_map(Edit::RawRotate),
        1 => proptest::collection::vec(0_u32..12, 0..8).prop_map(Edit::Set),
        1 => proptest::collection::vec(0_u32..12, 0..8).prop_map(Edit::Replace),
        2 => (0_u32..5).prop_map(Edit::Tick),
    ]
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        8 => edit().prop_map(Step::Edit),
        5 => proptest::collection::vec(edit(), 1..7).prop_map(Step::Txn),
        2 => proptest::collection::vec(edit(), 1..4).prop_map(Step::Abort),
        2 => (0_u32..3).prop_map(Step::ObserveOn),
        2 => (0_u32..3).prop_map(Step::ObserveOff),
        1 => Just(Step::ObserveAllOn),
        1 => Just(Step::ObserveAllOff),
    ]
}

/// Drives a script against the core and the mirror and checks, after every step, that the host's
/// list equals the core's whenever the host can know it.
fn run_script(steps: &[Step]) {
    let core = core();
    let mut mirror = Mirror::default();
    // The list has a live baseline on the core side: the next commit of it can be recorded.
    let mut healthy = false;
    // An abandoned commit happened and nothing has resynchronised the list since.
    let mut diverged = false;

    let observe_step =
        |mirror: &mut Mirror, healthy: &mut bool, diverged: &mut bool, id: u32, on: bool| {
            let entries = observe(&core.cell, id, on);
            let ids: Vec<u32> = if id == ALL_SIGNALS {
                (0..3).collect()
            } else {
                vec![id]
            };
            for i in &ids {
                mirror.observed[*i as usize] = on;
            }
            for e in &entries {
                assert_eq!(e.op, ChangeOp::Full, "observe sends full values");
                mirror.apply_entry(e);
            }
            if ids.contains(&LIST) {
                *healthy = on;
                if on {
                    *diverged = false;
                }
            }
        };
    observe_step(&mut mirror, &mut healthy, &mut diverged, ALL_SIGNALS, true);

    for step in steps {
        // The edits that run in this step, and whether they all are recorded operations.
        let (edits, aborting): (&[Edit], bool) = match step {
            Step::Edit(e) => (std::slice::from_ref(e), false),
            Step::Txn(es) => (es, false),
            Step::Abort(es) => (es, true),
            Step::ObserveOn(id) => {
                observe_step(&mut mirror, &mut healthy, &mut diverged, *id, true);
                (&[], false)
            }
            Step::ObserveOff(id) => {
                observe_step(&mut mirror, &mut healthy, &mut diverged, *id, false);
                (&[], false)
            }
            Step::ObserveAllOn => {
                observe_step(&mut mirror, &mut healthy, &mut diverged, ALL_SIGNALS, true);
                (&[], false)
            }
            Step::ObserveAllOff => {
                observe_step(&mut mirror, &mut healthy, &mut diverged, ALL_SIGNALS, false);
                (&[], false)
            }
        };

        if !edits.is_empty() {
            let expect_recorded = healthy && mirror.observed[LIST as usize] && !diverged;
            let mut effects = Vec::new();
            core.armed.store(aborting, Ordering::SeqCst);
            let result = catch_unwind(AssertUnwindSafe(|| {
                with_sink(core.sink.clone(), || {
                    txn(|| {
                        for e in edits {
                            effects.push(core.edit(e));
                        }
                    });
                });
            }));
            core.armed.store(false, Ordering::SeqCst);
            let sets = core.sink.take_decoded();

            let touched_list = effects.iter().any(|e| *e != Effect::NotTheList);
            let aborted = result.is_err();
            if aborted {
                assert!(aborting, "only the armed computed can abort a commit");
                assert!(sets.is_empty(), "an abandoned commit sends nothing");
                healthy = false;
                diverged = mirror.observed[LIST as usize];
            } else {
                // The claim: a healthy list written only by recorded operations is sent as
                // exactly those operations.
                let only_recorded = effects.iter().all(|e| !matches!(e, Effect::Raw));
                if expect_recorded && touched_list && only_recorded {
                    let recorded: usize = effects
                        .iter()
                        .map(|e| match e {
                            Effect::Recorded(n) => *n,
                            _ => 0,
                        })
                        .sum();
                    assert_eq!(sets.len(), 1);
                    let e = entry(&sets[0], LIST);
                    assert_eq!(e.op, ChangeOp::KeyedPatch, "recorded ops are a patch");
                    assert_eq!(
                        patch_of::<Todo>(e).len(),
                        recorded,
                        "one op per recorded operation, O(ops)"
                    );
                }
                for set in &sets {
                    for e in &set.entries {
                        if e.signal_id == LIST {
                            healthy = true;
                        }
                    }
                }
            }
            for set in &sets {
                mirror.apply_set(set);
            }

            if aborted {
                // The next commit that touches the store resynchronises what the abort held back.
                with_sink(core.sink.clone(), || {
                    core.list.push(todo(9_999, "heal", false))
                });
                let sets = core.sink.take_decoded();
                if mirror.observed[LIST as usize] {
                    let e = sets
                        .iter()
                        .flat_map(|s| &s.entries)
                        .find(|e| e.signal_id == LIST)
                        .expect("the abandoned list is sent again");
                    assert_eq!(
                        e.op,
                        ChangeOp::Full,
                        "after an abort the list is sent in full"
                    );
                    healthy = true;
                    diverged = false;
                }
                for set in &sets {
                    mirror.apply_set(set);
                }
            }
        }

        if mirror.observed[LIST as usize] && !diverged {
            assert_eq!(mirror.list, core.list.get(), "after {step:?}");
        }
    }

    // A final resynchronisation always brings the host up to date.
    observe_step(&mut mirror, &mut healthy, &mut diverged, ALL_SIGNALS, true);
    assert_eq!(mirror.list, core.list.get());
}

fn cases() -> u32 {
    std::env::var("UNDRA_KEYED_OPS_CASES")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(1_500)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), ..ProptestConfig::default() })]

    #[test]
    fn the_host_mirror_agrees_with_the_core_under_recorded_operations(
        steps in proptest::collection::vec(step(), 1..40)
    ) {
        run_script(&steps);
    }
}

#[test]
fn a_fixed_script_through_every_path() {
    use Edit::*;
    run_script(&[
        Step::Edit(Push(7)),
        Step::Txn(vec![
            Insert(1, 8),
            UpdateAt(0),
            Move(0, 2),
            Remove(1),
            Tick(1),
        ]),
        Step::Txn(vec![Clear, Push(1), RawToggle(0), Push(2)]),
        Step::Abort(vec![Push(3), Remove(0)]),
        Step::Edit(Push(4)),
        Step::ObserveOff(LIST),
        Step::Edit(Push(5)),
        Step::Edit(Clear),
        Step::ObserveOn(LIST),
        Step::Edit(Push(6)),
        Step::Txn(vec![Replace(vec![6, 7]), Push(8)]),
        Step::Txn(vec![Push(9), Move(0, 2), UpdateAt(1)]),
        Step::ObserveAllOff,
        Step::Edit(Push(10)),
        Step::ObserveAllOn,
        Step::Edit(Move(0, 1)),
    ]);
}
