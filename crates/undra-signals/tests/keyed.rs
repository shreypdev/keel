//! Keyed lists: patches when possible, full values otherwise.

mod common;

use common::*;
use undra_signals::{Computed, Signal, txn};
use undra_wire::payload::{ChangeOp, ChangeSet};
use undra_wire::{Decode, PatchOp, Writer};

/// A store with `list` (0, keyed) and `tag` (1, plain).
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
    Fixture { rig, list, tag }
}

impl Fixture {
    /// Runs `f` and returns the single change-set it produced.
    fn commit(&self, f: impl FnOnce()) -> ChangeSet {
        self.rig.run(f);
        self.rig.one_set()
    }
}

/// One edit of the list under test.
type Step = Box<dyn Fn(&mut Vec<Todo>)>;

fn insert(index: u32, item: Todo) -> PatchOp<Todo> {
    PatchOp::Insert { index, item }
}

#[test]
fn appending_one_item_is_a_patch_with_one_insert() {
    let f = fixture(todos(5));
    f.rig.observe_on(0);
    let set = f.commit(|| f.list.update(|l| l.push(todo(6, "t6", false))));
    let patch = patch_of::<Todo>(entry(&set, 0));
    assert_eq!(patch.ops, vec![insert(5, todo(6, "t6", false))]);
}

#[test]
fn replacing_everything_sends_the_full_value() {
    let f = fixture(todos(5));
    f.rig.observe_on(0);
    let replacement: Vec<Todo> = (100..105).map(|id| todo(id, "new", false)).collect();
    let set = f.commit(|| f.list.set(replacement.clone()));
    let e = entry(&set, 0);
    assert_eq!(e.op, ChangeOp::Full);
    assert_eq!(value_of::<Vec<Todo>>(e), replacement);
}

#[test]
fn changing_one_item_is_a_patch_with_one_update() {
    let f = fixture(todos(5));
    f.rig.observe_on(0);
    let set = f.commit(|| f.list.update(|l| l[2].done = true));
    let patch = patch_of::<Todo>(entry(&set, 0));
    assert_eq!(
        patch.ops,
        vec![PatchOp::Update {
            index: 2,
            item: todo(3, "t3", true)
        }]
    );
}

#[test]
fn removing_one_item_is_a_patch_with_one_remove() {
    let f = fixture(todos(5));
    f.rig.observe_on(0);
    let set = f.commit(|| f.list.update(|l| drop(l.remove(1))));
    assert_eq!(
        patch_of::<Todo>(entry(&set, 0)).ops,
        vec![PatchOp::Remove { index: 1 }]
    );
}

#[test]
fn removing_more_than_half_falls_back_to_the_full_value() {
    let f = fixture(todos(10));
    f.rig.observe_on(0);
    let set = f.commit(|| f.list.update(|l| l.truncate(4)));
    let e = entry(&set, 0);
    assert_eq!(e.op, ChangeOp::Full);
    assert_eq!(value_of::<Vec<Todo>>(e), todos(4));
}

#[test]
fn removing_exactly_half_is_still_a_patch() {
    let f = fixture(todos(10));
    f.rig.observe_on(0);
    let set = f.commit(|| f.list.update(|l| l.truncate(5)));
    assert_eq!(entry(&set, 0).op, ChangeOp::KeyedPatch);
}

#[test]
fn clearing_the_list_sends_the_full_value() {
    let f = fixture(todos(3));
    f.rig.observe_on(0);
    let set = f.commit(|| f.list.set(Vec::new()));
    let e = entry(&set, 0);
    assert_eq!(e.op, ChangeOp::Full);
    assert!(value_of::<Vec<Todo>>(e).is_empty());
}

#[test]
fn reordering_is_a_move() {
    let f = fixture(todos(6));
    f.rig.observe_on(0);
    let set = f.commit(|| f.list.update(|l| l.rotate_left(1)));
    assert_eq!(
        patch_of::<Todo>(entry(&set, 0)).ops,
        vec![PatchOp::Move { from: 0, to: 5 }]
    );
}

#[test]
fn inserting_in_the_middle_and_at_the_front() {
    let f = fixture(todos(3));
    f.rig.observe_on(0);
    let set = f.commit(|| f.list.update(|l| l.insert(1, todo(50, "mid", false))));
    assert_eq!(
        patch_of::<Todo>(entry(&set, 0)).ops,
        vec![insert(1, todo(50, "mid", false))]
    );
    let set = f.commit(|| f.list.update(|l| l.insert(0, todo(60, "head", false))));
    assert_eq!(
        patch_of::<Todo>(entry(&set, 0)).ops,
        vec![insert(0, todo(60, "head", false))]
    );
}

#[test]
fn an_unchanged_list_is_an_empty_patch() {
    let f = fixture(todos(3));
    f.rig.observe_on(0);
    let set = f.commit(|| f.list.set(todos(3)));
    let patch = patch_of::<Todo>(entry(&set, 0));
    assert!(patch.is_empty());
}

#[test]
fn observing_sets_the_baseline_so_the_first_change_is_already_a_patch() {
    let f = fixture(todos(4));
    let first = f.rig.observe_on(0);
    assert_eq!(first[0].op, ChangeOp::Full);
    assert_eq!(value_of::<Vec<Todo>>(&first[0]), todos(4));
    let set = f.commit(|| f.list.update(|l| l.push(todo(9, "x", true))));
    assert_eq!(entry(&set, 0).op, ChangeOp::KeyedPatch);
}

#[test]
fn a_write_before_observing_is_part_of_the_baseline() {
    let f = fixture(todos(4));
    f.rig
        .run(|| f.list.update(|l| l.push(todo(5, "early", false))));
    let entries = f.rig.observe_on(0);
    assert_eq!(value_of::<Vec<Todo>>(&entries[0]).len(), 5);
    let set = f.commit(|| f.list.update(|l| l.push(todo(6, "late", false))));
    assert_eq!(
        patch_of::<Todo>(entry(&set, 0)).ops,
        vec![insert(5, todo(6, "late", false))],
        "the patch is relative to what observe sent, not to the construction-time list"
    );
}

#[test]
fn re_observing_resets_the_baseline() {
    let f = fixture(todos(4));
    f.rig.observe_on(0);
    f.commit(|| f.list.update(|l| l.push(todo(5, "a", false))));
    // The host lost track and asks again; it gets the full value...
    let again = f.rig.observe_on(0);
    assert_eq!(again[0].op, ChangeOp::Full);
    assert_eq!(value_of::<Vec<Todo>>(&again[0]).len(), 5);
    // ...and the next patch is relative to it.
    let set = f.commit(|| f.list.update(|l| l.push(todo(6, "b", false))));
    assert_eq!(
        patch_of::<Todo>(entry(&set, 0)).ops,
        vec![insert(5, todo(6, "b", false))]
    );
}

#[test]
fn observe_off_then_on_starts_from_a_fresh_baseline() {
    let f = fixture(todos(4));
    f.rig.observe_on(0);
    f.rig.observe_off(0);
    f.rig
        .run(|| f.list.update(|l| l.push(todo(5, "missed", false))));
    assert!(f.rig.sets().is_empty());
    let entries = f.rig.observe_on(0);
    assert_eq!(entries[0].op, ChangeOp::Full);
    assert_eq!(value_of::<Vec<Todo>>(&entries[0]).len(), 5);
    let set = f.commit(|| f.list.update(|l| l.push(todo(6, "seen", false))));
    assert_eq!(
        patch_of::<Todo>(entry(&set, 0)).ops,
        vec![insert(5, todo(6, "seen", false))]
    );
}

#[test]
fn duplicate_keys_fall_back_to_the_full_value() {
    let f = fixture(todos(3));
    f.rig.observe_on(0);
    let set = f.commit(|| f.list.update(|l| l.push(todo(1, "dup", false))));
    let e = entry(&set, 0);
    assert_eq!(e.op, ChangeOp::Full);
    assert_eq!(value_of::<Vec<Todo>>(e).len(), 4);
    // The baseline itself still has the duplicate, so removing it is a full value too...
    let set = f.commit(|| f.list.update(|l| drop(l.pop())));
    assert_eq!(entry(&set, 0).op, ChangeOp::Full);
    // ...after which both sides are unique again and patches resume.
    let set = f.commit(|| f.list.update(|l| l.push(todo(9, "ok", false))));
    assert_eq!(entry(&set, 0).op, ChangeOp::KeyedPatch);
}

#[test]
fn the_first_item_added_to_an_empty_list_is_a_full_value() {
    let f = fixture(Vec::new());
    f.rig.observe_on(0);
    let set = f.commit(|| f.list.update(|l| l.push(todo(1, "first", false))));
    assert_eq!(entry(&set, 0).op, ChangeOp::Full);
    let set = f.commit(|| f.list.update(|l| l.push(todo(2, "second", false))));
    assert_eq!(entry(&set, 0).op, ChangeOp::KeyedPatch);
}

#[test]
fn several_operations_in_one_transaction_make_one_patch() {
    let f = fixture(todos(6));
    f.rig.observe_on(0);
    let set = f.commit(|| {
        txn(|| {
            f.list.update(|l| l.push(todo(7, "seven", false)));
            f.list.update(|l| drop(l.remove(0)));
            f.list.update(|l| l[0].done = true);
        });
    });
    assert_eq!(set.entries.len(), 1);
    let patch = patch_of::<Todo>(entry(&set, 0));
    let mut replica = todos(6);
    patch.apply(&mut replica).unwrap();
    assert_eq!(replica, f.list.get());
}

#[test]
fn a_replica_that_applies_every_change_stays_equal() {
    let f = fixture(todos(8));
    let mut replica = value_of::<Vec<Todo>>(&f.rig.observe_on(0)[0]);
    let steps: Vec<Step> = vec![
        Box::new(|l| l.push(todo(20, "a", false))),
        Box::new(|l| l.swap(0, 3)),
        Box::new(|l| l[1].title = "renamed".into()),
        Box::new(|l| drop(l.remove(2))),
        Box::new(|l| l.insert(4, todo(21, "b", true))),
        Box::new(|l| l.reverse()),
        Box::new(|l| l.retain(|t| t.id % 2 == 0)),
        Box::new(|l| l.extend((30..34).map(|id| todo(id, "c", false)))),
        Box::new(|l| l.rotate_right(2)),
        Box::new(|l| l.clear()),
        Box::new(|l| l.extend(todos(3))),
    ];
    for step in &steps {
        let set = f.commit(|| f.list.update(|l| step(l)));
        let e = entry(&set, 0);
        match e.op {
            ChangeOp::Full => replica = value_of(e),
            ChangeOp::KeyedPatch => patch_of::<Todo>(e).apply(&mut replica).unwrap(),
            other => panic!("unexpected op {other:?}"),
        }
        assert_eq!(replica, f.list.get());
    }
}

#[test]
fn writes_while_unobserved_and_no_coalesce_send_the_full_value() {
    let f = fixture(todos(3));
    f.rig.cell.set_no_coalesce(0).unwrap();
    let set = f.commit(|| f.list.update(|l| l.push(todo(9, "n", false))));
    let e = entry(&set, 0);
    assert_eq!(e.op, ChangeOp::Full, "no baseline exists while unobserved");
    assert_eq!(value_of::<Vec<Todo>>(e).len(), 4);
}

#[test]
fn encode_signal_and_snapshot_use_the_full_value() {
    let f = fixture(todos(3));
    let mut w = Writer::new();
    assert!(f.rig.cell.encode_signal(0, &mut w));
    assert_eq!(Vec::<Todo>::decode_exact(w.as_slice()).unwrap(), todos(3));
    let mut snapshot = Writer::new();
    f.rig.cell.encode_snapshot(&mut snapshot);
    assert!(!snapshot.is_empty());
}

#[test]
fn a_large_list_with_one_new_item_sends_a_tiny_patch() {
    let f = fixture(todos(10_000));
    let first = f.rig.observe_on(0);
    let full_len = first[0].value.len();
    let set = f.commit(|| f.list.update(|l| l.push(todo(10_001, "new", false))));
    let e = entry(&set, 0);
    assert_eq!(patch_of::<Todo>(e).len(), 1);
    assert!(
        e.value.len() * 1000 < full_len,
        "patch {} vs full {}",
        e.value.len(),
        full_len
    );
}

#[test]
fn plain_and_keyed_signals_share_a_change_set() {
    let f = fixture(todos(3));
    f.rig.observe_all();
    let set = f.commit(|| {
        txn(|| {
            f.tag.set(7);
            f.list.update(|l| l.push(todo(4, "x", false)));
        });
    });
    assert_eq!(entry(&set, 0).op, ChangeOp::KeyedPatch);
    assert_eq!(entry(&set, 1).op, ChangeOp::Full);
    assert_eq!(value_of::<u32>(entry(&set, 1)), 7);
}

#[test]
fn a_computed_over_a_keyed_list_is_delivered_as_a_full_value() {
    let rig = Rig::new();
    let list = Signal::new(todos(3));
    let open = Computed::new(&list, |l: &Vec<Todo>| {
        u32::try_from(l.iter().filter(|t| !t.done).count()).unwrap()
    });
    rig.cell.attach_keyed(&list, 0, todo_key).unwrap();
    rig.cell.attach_computed(&open, 1).unwrap();
    rig.observe_all();
    rig.run(|| list.update(|l| l[0].done = true));
    let set = rig.one_set();
    assert_eq!(entry(&set, 0).op, ChangeOp::KeyedPatch);
    assert_eq!(entry(&set, 1).op, ChangeOp::Full);
    assert_eq!(value_of::<u32>(entry(&set, 1)), 2);
}

#[test]
fn identical_consecutive_commits_each_send_an_empty_patch() {
    let f = fixture(todos(2));
    f.rig.observe_on(0);
    for _ in 0..3 {
        let set = f.commit(|| f.list.set(todos(2)));
        assert!(patch_of::<Todo>(entry(&set, 0)).is_empty());
    }
}

#[test]
fn unicode_titles_survive_patches() {
    let f = fixture(vec![todo(1, "café", false), todo(2, "日本語", false)]);
    f.rig.observe_on(0);
    let set = f.commit(|| f.list.update(|l| l.push(todo(3, "🚀 launch", true))));
    let patch = patch_of::<Todo>(entry(&set, 0));
    assert_eq!(patch.ops, vec![insert(2, todo(3, "🚀 launch", true))]);
}
