//! The write-context check (`set_write_checker`): review finding M2, and ADR-035, which makes it
//! hold **in every build** and asks it about the store's owning runtime.
//!
//! The checker is process-global, so this file is its own test binary; each test decides what
//! "the core" is for its own thread through a thread-local (the runtime ids it "holds"), so the
//! tests can run in parallel. None of these tests is `#[cfg(debug_assertions)]`: CI runs this file
//! in both profiles (`cargo test --release`).

mod common;

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Once};

use common::*;
use parking_lot::Mutex;
use undra_signals::{
    ChangeSink, Computed, Signal, StoreCell, WriteError, set_write_checker, with_sink,
};

thread_local! {
    /// The runtimes whose core lock the current thread pretends to hold; `None` means "all of
    /// them" (the default, so fixtures can set things up freely).
    static HOLDS: RefCell<Option<Vec<u64>>> = const { RefCell::new(None) };
}

fn checker(owner: u64) -> bool {
    HOLDS.with(|holds| match &*holds.borrow() {
        None => true,
        Some(ids) if owner == 0 => !ids.is_empty(),
        Some(ids) => ids.contains(&owner),
    })
}

fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| set_write_checker(checker));
}

/// Runs `f` as a thread that holds exactly the core locks of `ids`.
fn holding<R>(ids: &[u64], f: impl FnOnce() -> R) -> R {
    HOLDS.with(|holds| *holds.borrow_mut() = Some(ids.to_vec()));
    let result = catch_unwind(AssertUnwindSafe(f));
    HOLDS.with(|holds| *holds.borrow_mut() = None);
    match result {
        Ok(r) => r,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default()
}

/// A rig whose store is owned by runtime `owner`.
fn owned_rig(owner: u64) -> Rig {
    let rig = Rig::new();
    rig.cell.set_owner(owner);
    rig
}

#[test]
fn m2_ow1_a_write_from_a_thread_off_the_core_is_refused_in_every_build() {
    install();
    let rig = owned_rig(5);
    let x = Signal::new(0_u32);
    rig.cell.attach(&x, 0).unwrap();
    rig.observe_all();

    // The audit's repro (OW-1): a thread that holds no core lock (a blocking-pool worker, a host
    // thread). In a release build it used to be applied and never delivered.
    let result = catch_unwind(AssertUnwindSafe(|| holding(&[], || rig.run(|| x.set(1)))));
    let message = panic_text(&*result.expect_err("the write must be refused, in every build"));
    assert!(
        message.starts_with("error[undra::E0065]: a signal of a store owned by runtime 5"),
        "unhelpful message: {message}"
    );
    assert!(message.contains("= help:") && message.contains("with_core"));
    assert!(message.ends_with("errors.html#E0065"), "{message}");
    // The shape of every diagnostic (SPEC section 12), and the golden the error-codes page of the
    // site shows: regenerate it with `UPDATE_GOLDEN=1 cargo test -p undra-signals --test
    // write_checker` and review the diff.
    let lines: Vec<&str> = message.lines().collect();
    assert_eq!(lines.len(), 4, "{message}");
    assert!(lines[1].starts_with("  = note: ") && lines[2].starts_with("  = help: "));
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/diagnostics/E0065.txt");
    if std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{message}\n")).unwrap();
    }
    let golden = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run with UPDATE_GOLDEN=1", path.display()));
    assert_eq!(golden, format!("{message}\n"));

    // Refused before anything changed: the value, the transaction state and delivery are intact.
    assert_eq!(x.get(), 0);
    assert!(rig.sets().is_empty());
    rig.run(|| x.set(2));
    assert_eq!(value_of::<u32>(entry(&rig.one_set(), 0)), 2);
}

#[test]
fn ow2_the_core_of_another_runtime_may_not_write_a_store_it_does_not_own() {
    install();
    let rig = owned_rig(5);
    let x = Signal::new(0_u32);
    rig.cell.attach(&x, 0).unwrap();
    rig.observe_all();
    // Holding *a* core lock used to be enough; it must be the owner's.
    let refused = catch_unwind(AssertUnwindSafe(|| holding(&[6], || rig.run(|| x.set(1)))));
    assert!(refused.is_err());
    assert_eq!(x.get(), 0);
    holding(&[6, 5], || rig.run(|| x.set(3)));
    assert_eq!(value_of::<u32>(entry(&rig.one_set(), 0)), 3);
}

#[test]
fn list_operations_and_update_are_checked_like_set() {
    install();
    let rig = owned_rig(5);
    let list = Signal::new(vec![1_u32, 2]);
    rig.cell
        .attach_keyed(&list, 0, |n: &u32| u64::from(*n))
        .unwrap();
    rig.observe_all();
    for attempt in [
        &(|| list.push(3)) as &dyn Fn(),
        &|| list.update(|v| v.push(4)),
        &|| {
            list.remove(0);
        },
        &|| list.clear(),
        &|| list.replace(vec![9]),
    ] {
        let refused = catch_unwind(AssertUnwindSafe(|| holding(&[], || rig.run(attempt))));
        assert!(refused.is_err());
    }
    assert_eq!(list.get(), vec![1, 2], "nothing was written");
    assert!(rig.sets().is_empty());
}

#[test]
fn try_set_and_try_update_return_the_refusal_instead_of_panicking() {
    install();
    let rig = owned_rig(9);
    let x = Signal::new(1_u32);
    rig.cell.attach(&x, 0).unwrap();
    rig.observe_all();
    holding(&[], || {
        assert!(!x.can_write());
        assert_eq!(x.try_set(2), Err(WriteError::OffCore { owner: 9 }));
        let mut called = false;
        assert_eq!(
            x.try_update(|v| {
                called = true;
                *v = 3;
            }),
            Err(WriteError::OffCore { owner: 9 })
        );
        assert!(!called, "the closure does not run for a refused write");
    });
    assert_eq!(x.get(), 1);
    holding(&[9], || {
        assert!(x.can_write());
        rig.run(|| {
            assert_eq!(x.try_set(4), Ok(()));
            assert_eq!(x.try_update(|v| *v += 1), Ok(()));
        });
    });
    assert_eq!(x.get(), 5);
    assert_eq!(rig.sets().len(), 2);
}

#[test]
fn m2_a_write_that_reaches_an_attached_computed_is_checked_too() {
    install();
    let rig = owned_rig(5);
    let local = Signal::new(1_u32); // not attached itself
    let derived = Computed::new(&local, |v: &u32| v * 2);
    rig.cell.attach_computed(&derived, 0).unwrap();
    rig.observe_all();
    // An unattached signal with dependents belongs to no store: any core lock will do, none won't.
    let result = catch_unwind(AssertUnwindSafe(|| holding(&[], || local.set(5))));
    assert!(result.is_err());
    assert_eq!(local.get(), 1);
    assert_eq!(local.try_set(6), Ok(()), "the test thread holds every lock");
    holding(&[1], || rig.run(|| local.set(7)));
    assert_eq!(local.get(), 7);
}

#[test]
fn m2_a_purely_local_signal_may_be_written_from_anywhere() {
    install();
    // Unattached, and nothing depends on it: there is nothing to deliver and nobody to race.
    let scratch = Signal::new(0_u32);
    holding(&[], || {
        assert!(scratch.can_write());
        scratch.set(9);
    });
    assert_eq!(scratch.get(), 9);
}

#[test]
fn m2_writes_on_the_core_pass() {
    install();
    let rig = owned_rig(5);
    let x = Signal::new(0_u32);
    rig.cell.attach(&x, 0).unwrap();
    rig.observe_all();
    holding(&[5], || rig.run(|| x.set(1)));
    assert_eq!(rig.one_set().entries.len(), 1);
}

/// A sink that records which runtime each change-set is for, and the refusals it hears of.
#[derive(Default)]
struct Routing {
    delivered: Mutex<Vec<(u64, Vec<u8>)>>,
    refused: Mutex<Vec<(u64, String)>>,
}

impl ChangeSink for Routing {
    fn deliver(&self, _: &[u8]) {
        panic!("the commit must name the store's owner (deliver_from)");
    }

    fn deliver_from(&self, owner: u64, change_set: &[u8]) {
        self.delivered.lock().push((owner, change_set.to_vec()));
    }

    fn off_core_write(&self, owner: u64, message: &str) {
        self.refused.lock().push((owner, message.to_owned()));
    }
}

#[test]
fn ow2_commits_name_the_owner_of_each_store_and_refusals_are_reported_to_the_sink() {
    install();
    let a = StoreCell::new(1);
    let b = StoreCell::new(2);
    a.set_owner(11);
    b.set_owner(22);
    a.set_handle(0x1_0000_0001);
    b.set_handle(0x1_0000_0002);
    let (x, y) = (Signal::new(0_u8), Signal::new(0_u8));
    a.attach(&x, 0).unwrap();
    b.attach(&y, 0).unwrap();
    observe(&a, 0, true);
    observe(&b, 0, true);
    assert_eq!((a.owner(), b.owner()), (11, 22));

    let sink = Arc::new(Routing::default());
    with_sink(sink.clone(), || {
        holding(&[11, 22], || {
            undra_signals::txn(|| {
                x.set(1);
                y.set(2);
            });
        });
    });
    let owners: Vec<u64> = sink.delivered.lock().iter().map(|(o, _)| *o).collect();
    assert_eq!(
        owners,
        [11, 22],
        "one change-set per store, each to its owner"
    );

    let refused = catch_unwind(AssertUnwindSafe(|| {
        with_sink(sink.clone(), || holding(&[22], || x.set(3)));
    }));
    assert!(refused.is_err());
    let reports = sink.refused.lock().clone();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].0, 11);
    assert!(reports[0].1.contains("E0065"), "{}", reports[0].1);
    assert_eq!(x.get(), 1);
}

#[test]
fn a_lazy_list_of_a_store_follows_the_same_write_rule_as_a_signal() {
    use undra_signals::Lazy;
    install();
    let rig = owned_rig(5);
    let list = Lazy::from_vec(vec![1_u32, 2, 3]);
    rig.cell.attach_lazy(&list, 0).unwrap();
    rig.observe_all();

    // Off the core: every write method refuses (E0065) and nothing changes, not even the version.
    holding(&[], || {
        assert!(!list.can_write());
        let refuse = |f: &dyn Fn()| {
            let result = catch_unwind(AssertUnwindSafe(|| rig.run(f)));
            let message = panic_text(&*result.expect_err("the write must be refused"));
            assert!(message.contains("E0065"), "{message}");
        };
        refuse(&|| list.push(4));
        refuse(&|| list.insert(0, 4));
        refuse(&|| {
            let _ = list.remove(0);
        });
        refuse(&|| list.update_at(0, |n| *n = 9));
        refuse(&|| list.move_item(0, 1));
        refuse(&|| list.replace(vec![7]));
        refuse(&|| list.clear());
    });
    assert_eq!((list.to_vec(), list.version()), (vec![1, 2, 3], 0));
    assert!(rig.sets().is_empty());

    // On the core it is written and announced.
    holding(&[5], || {
        assert!(list.can_write());
        rig.run(|| list.push(4));
    });
    let set = rig.one_set();
    assert_eq!(
        set.entries[0].op,
        undra_wire::payload::ChangeOp::LazyInvalidated
    );
}
