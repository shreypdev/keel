//! The ledger of references the core hands to the host (ADR-040): a scope gives back what its call
//! did not carry, an object has one handle however often it is issued, and the table's count of
//! host references is exactly what a model host owns, whatever order issues, rollbacks, panics,
//! releases and restores come in.

use std::sync::Arc;

use proptest::prelude::*;
use undra_runtime::testing::TestRuntime;
use undra_runtime::{Handle, UndraObject};

struct Thing(#[allow(dead_code)] u32);

impl UndraObject for Thing {
    const TYPE_ID: u32 = 0x7001;
    const NAME: &'static str = "Thing";
}

fn things(n: u32) -> Vec<Arc<Thing>> {
    (0..n).map(|i| Arc::new(Thing(i))).collect()
}

#[test]
fn an_uncommitted_scope_gives_everything_back_and_a_committed_one_keeps_it() {
    let t = TestRuntime::new();
    let rt = t.runtime();
    let all = things(2);

    let mut scope = rt.issue_scope();
    let a = scope.issue(all[0].clone()).unwrap();
    let b = scope.issue(all[1].clone()).unwrap();
    assert_eq!(scope.len(), 2);
    assert_eq!(rt.objects().host_refs(), 2);
    drop(scope);
    assert_eq!(
        rt.objects().host_refs(),
        0,
        "nothing carried them: nobody owns them"
    );
    assert!(rt.objects().host_refs_of(a).is_none() && rt.objects().host_refs_of(b).is_none());
    assert_eq!(rt.objects().live(), 0);

    let mut scope = rt.issue_scope();
    let a = scope.issue(all[0].clone()).unwrap();
    scope.commit();
    assert_eq!(rt.objects().host_refs_of(a), Some(1));
    assert_eq!(rt.objects().live(), 1);
}

#[test]
fn a_rolled_back_second_issue_of_a_held_object_only_gives_its_own_reference_back() {
    let t = TestRuntime::new();
    let rt = t.runtime();
    let thing = Arc::new(Thing(1));
    let mut scope = rt.issue_scope();
    let held = scope.issue(thing.clone()).unwrap();
    scope.commit();

    let mut scope = rt.issue_scope();
    assert_eq!(scope.issue(thing.clone()).unwrap(), held);
    assert_eq!(rt.objects().host_refs_of(held), Some(2));
    drop(scope);
    assert_eq!(
        rt.objects().host_refs_of(held),
        Some(1),
        "the host's own reference stays"
    );
}

#[test]
fn a_panic_after_some_issues_gives_them_back_while_it_unwinds() {
    let t = TestRuntime::new();
    let rt = t.runtime().clone();
    let all = things(3);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut scope = rt.issue_scope();
        scope.issue(all[0].clone()).unwrap();
        scope.issue(all[1].clone()).unwrap();
        panic!("the lowering of the third failed");
    }));
    assert!(caught.is_err());
    assert_eq!(rt.objects().host_refs(), 0);
    assert_eq!(rt.objects().live(), 0);
}

#[test]
fn a_restore_makes_every_plain_handle_stale_issued_or_constructed() {
    let t = TestRuntime::new();
    let rt = t.runtime();
    // Neither is a store, so a snapshot holds nothing either way: the difference is in the table.
    let thing = Arc::new(Thing(1));
    let mut scope = rt.issue_scope();
    let derived = scope.issue(thing.clone()).unwrap();
    let constructed = scope.issue_constructed(Arc::new(Thing(2))).unwrap();
    scope.commit();
    assert_ne!(derived, constructed);
    rt.restore(&rt.snapshot()).unwrap();
    assert!(
        rt.objects().host_refs_of(derived).is_none(),
        "a restore makes every plain handle stale"
    );
    assert_eq!(rt.objects().host_refs(), 0);
}

#[derive(Clone, Debug)]
enum Op {
    Issue(usize),
    Rollback(usize),
    Panic(usize, usize),
    Release(usize),
    Restore,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0..5_usize).prop_map(Op::Issue),
        2 => (0..5_usize).prop_map(Op::Rollback),
        1 => (0..5_usize, 0..5_usize).prop_map(|(a, b)| Op::Panic(a, b)),
        4 => (0..5_usize).prop_map(Op::Release),
        1 => Just(Op::Restore),
    ]
}

proptest! {
    /// Random sequences of issue, rollback, panic, release and restore never leave `host_refs`
    /// different from the number of references the model host owns, and an object has one handle
    /// for as long as the host holds it.
    #[test]
    fn host_refs_always_equal_what_the_model_host_owns(ops in proptest::collection::vec(op(), 1..60)) {
        let t = TestRuntime::new();
        let rt = t.runtime().clone();
        let all = things(5);
        let mut refs = [0_u32; 5];
        let mut handles: [Option<Handle>; 5] = [None; 5];
        for op in ops {
            match op {
                Op::Issue(i) => {
                    let mut scope = rt.issue_scope();
                    let h = scope.issue(all[i].clone()).unwrap();
                    scope.commit();
                    match handles[i] {
                        Some(held) => prop_assert_eq!(h, held, "one handle while the host holds it"),
                        None => handles[i] = Some(h),
                    }
                    refs[i] += 1;
                }
                Op::Rollback(i) => {
                    let mut scope = rt.issue_scope();
                    let h = scope.issue(all[i].clone()).unwrap();
                    if let Some(held) = handles[i] {
                        prop_assert_eq!(h, held);
                    }
                    drop(scope);
                }
                Op::Panic(i, j) => {
                    let (a, b) = (all[i].clone(), all[j].clone());
                    let rt2 = rt.clone();
                    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                        let mut scope = rt2.issue_scope();
                        scope.issue(a).unwrap();
                        scope.issue(b).unwrap();
                        panic!("lowering failed");
                    }));
                    prop_assert!(caught.is_err());
                }
                Op::Release(i) => {
                    if let Some(held) = handles[i] {
                        rt.release(held.0);
                        refs[i] -= 1;
                        if refs[i] == 0 {
                            handles[i] = None;
                        }
                    }
                }
                Op::Restore => {
                    rt.restore(&rt.snapshot()).unwrap();
                    refs = [0; 5];
                    handles = [None; 5];
                }
            }
            let model: u64 = refs.iter().map(|&n| u64::from(n)).sum();
            prop_assert_eq!(rt.objects().host_refs(), model);
            for i in 0..5 {
                match handles[i] {
                    Some(held) => prop_assert_eq!(rt.objects().host_refs_of(held), Some(refs[i])),
                    None => prop_assert_eq!(refs[i], 0),
                }
            }
            prop_assert_eq!(
                rt.objects().live(),
                handles.iter().filter(|h| h.is_some()).count()
            );
        }
    }
}
