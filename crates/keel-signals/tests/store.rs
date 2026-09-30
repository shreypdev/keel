//! `StoreCell`: attaching, observing and committing plain and computed signals.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use keel_signals::testing::CaptureSink;
use keel_signals::{
    ALL_SIGNALS, Computed, Signal, SignalsError, StoreCell, next_txn_id, txn, with_sink,
};
use keel_wire::payload::{ChangeOp, StoreSnapshot};
use keel_wire::{Reader, Writer};

/// `count` (0), `name` (1), `double` = count * 2 (2, computed), `label` = name + double (3,
/// computed of a signal and a computed).
struct Counter {
    rig: Rig,
    count: Signal<i32>,
    name: Signal<String>,
    double: Computed<i32>,
    label: Computed<String>,
    double_runs: Arc<AtomicUsize>,
    label_runs: Arc<AtomicUsize>,
}

fn counter() -> Counter {
    let rig = Rig::new();
    let count = Signal::new(1);
    let name = Signal::new(String::from("n"));
    let double_runs = Arc::new(AtomicUsize::new(0));
    let label_runs = Arc::new(AtomicUsize::new(0));
    let runs = double_runs.clone();
    let double = Computed::new(&count, move |c: &i32| {
        runs.fetch_add(1, Ordering::SeqCst);
        c * 2
    });
    let runs = label_runs.clone();
    let label = Computed::new((&name, &double), move |(n, d): (&String, &i32)| {
        runs.fetch_add(1, Ordering::SeqCst);
        format!("{n}{d}")
    });
    rig.cell.attach(&count, 0).unwrap();
    rig.cell.attach(&name, 1).unwrap();
    rig.cell.attach_computed(&double, 2).unwrap();
    rig.cell.attach_computed(&label, 3).unwrap();
    Counter {
        rig,
        count,
        name,
        double,
        label,
        double_runs,
        label_runs,
    }
}

// ---------------------------------------------------------------------------------------------
// attach
// ---------------------------------------------------------------------------------------------

#[test]
fn attach_in_order_counts_signals() {
    let c = counter();
    assert_eq!(c.rig.cell.signal_count(), 4);
    assert!(c.count.is_attached() && c.name.is_attached());
    assert!(c.double.is_attached() && c.label.is_attached());
}

#[test]
fn attaching_the_same_signal_twice_is_an_error() {
    let cell = StoreCell::new(1);
    let s = Signal::new(0_u8);
    cell.attach(&s, 0).unwrap();
    assert_eq!(cell.attach(&s, 1), Err(SignalsError::AlreadyAttached));
    assert_eq!(cell.attach(&s, 0), Err(SignalsError::AlreadyAttached));
    assert_eq!(cell.signal_count(), 1, "a failed attach adds nothing");
}

#[test]
fn a_clone_of_an_attached_signal_is_attached_too() {
    let cell = StoreCell::new(1);
    let s = Signal::new(0_u8);
    cell.attach(&s, 0).unwrap();
    let other = StoreCell::new(2);
    assert_eq!(
        other.attach(&s.clone(), 0),
        Err(SignalsError::AlreadyAttached)
    );
}

#[test]
fn a_signal_cannot_belong_to_two_stores() {
    let a = StoreCell::new(1);
    let b = StoreCell::new(2);
    let s = Signal::new(0_u8);
    a.attach(&s, 0).unwrap();
    assert_eq!(b.attach(&s, 0), Err(SignalsError::AlreadyAttached));
    assert_eq!(b.signal_count(), 0);
}

#[test]
fn signal_ids_must_be_attached_in_declaration_order() {
    let cell = StoreCell::new(1);
    let a = Signal::new(0_u8);
    let b = Signal::new(0_u8);
    assert_eq!(
        cell.attach(&a, 1),
        Err(SignalsError::OutOfOrder {
            expected: 0,
            got: 1
        })
    );
    assert!(
        !a.is_attached(),
        "a rejected attach must not bind the signal"
    );
    cell.attach(&a, 0).unwrap();
    assert_eq!(
        cell.attach(&b, 0),
        Err(SignalsError::OutOfOrder {
            expected: 1,
            got: 0
        })
    );
    assert_eq!(
        cell.attach(&b, ALL_SIGNALS),
        Err(SignalsError::OutOfOrder {
            expected: 1,
            got: ALL_SIGNALS
        })
    );
    cell.attach(&b, 1).unwrap();
    assert_eq!(cell.signal_count(), 2);
}

#[test]
fn attach_computed_and_keyed_report_the_same_errors() {
    let cell = StoreCell::new(1);
    let s = Signal::new(0_u8);
    let c = Computed::new(&s, |v| *v);
    assert_eq!(
        cell.attach_computed(&c, 3),
        Err(SignalsError::OutOfOrder {
            expected: 0,
            got: 3
        })
    );
    cell.attach_computed(&c, 0).unwrap();
    assert_eq!(
        cell.attach_computed(&c, 1),
        Err(SignalsError::AlreadyAttached)
    );

    let list = Signal::new(todos(1));
    assert_eq!(
        cell.attach_keyed(&list, 5, todo_key),
        Err(SignalsError::OutOfOrder {
            expected: 1,
            got: 5
        })
    );
    cell.attach_keyed(&list, 1, todo_key).unwrap();
    assert_eq!(
        cell.attach_keyed(&list, 2, todo_key),
        Err(SignalsError::AlreadyAttached)
    );
}

#[test]
fn set_no_coalesce_rejects_unknown_signals() {
    let cell = StoreCell::new(1);
    assert_eq!(
        cell.set_no_coalesce(0),
        Err(SignalsError::UnknownSignal { signal_id: 0 })
    );
    let s = Signal::new(0_u8);
    cell.attach(&s, 0).unwrap();
    assert_eq!(cell.set_no_coalesce(0), Ok(()));
    assert_eq!(
        cell.set_no_coalesce(1),
        Err(SignalsError::UnknownSignal { signal_id: 1 })
    );
}

#[test]
fn handle_and_type_id_round_trip() {
    let cell = StoreCell::new(0xDEAD_BEEF);
    assert_eq!(cell.type_id(), 0xDEAD_BEEF);
    assert_eq!(cell.handle(), 0);
    cell.set_handle(u64::MAX - 1);
    assert_eq!(cell.handle(), u64::MAX - 1);
}

// ---------------------------------------------------------------------------------------------
// observe
// ---------------------------------------------------------------------------------------------

#[test]
fn observe_on_emits_the_current_value_as_an_entry() {
    let c = counter();
    c.count.set(41);
    let entries = c.rig.observe_on(0);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].handle, handle());
    assert_eq!(entries[0].signal_id, 0);
    assert_eq!(entries[0].op, ChangeOp::Full);
    assert_eq!(value_of::<i32>(&entries[0]), 41);
    assert!(c.rig.cell.is_observed(0));
    assert!(!c.rig.cell.is_observed(1));
}

#[test]
fn observe_all_emits_every_signal_including_computeds_in_id_order() {
    let c = counter();
    let entries = c.rig.observe_all();
    let ids: Vec<u32> = entries.iter().map(|e| e.signal_id).collect();
    assert_eq!(ids, vec![0, 1, 2, 3]);
    assert_eq!(value_of::<i32>(&entries[0]), 1);
    assert_eq!(value_of::<String>(&entries[1]), "n");
    assert_eq!(value_of::<i32>(&entries[2]), 2);
    assert_eq!(value_of::<String>(&entries[3]), "n2");
    for id in 0..4 {
        assert!(c.rig.cell.is_observed(id));
    }
}

#[test]
fn observe_of_one_signal_leaves_the_others_unobserved() {
    let c = counter();
    c.rig.observe_on(1);
    c.rig.run(|| {
        c.count.set(5);
        c.name.set("x".into());
    });
    let set = c.rig.one_set();
    assert_eq!(ids(&set), vec![1]);
}

#[test]
fn observe_off_stops_delivery() {
    let c = counter();
    c.rig.observe_all();
    c.rig.run(|| c.count.set(2));
    assert_eq!(ids(&c.rig.one_set()), vec![0, 2, 3]);
    c.rig.observe_off(0);
    c.rig.run(|| c.count.set(3));
    assert_eq!(
        ids(&c.rig.one_set()),
        vec![2, 3],
        "0 is off, its computeds still on"
    );
    c.rig.observe_off(ALL_SIGNALS);
    c.rig.run(|| c.count.set(4));
    assert!(c.rig.sets().is_empty());
    for id in 0..4 {
        assert!(!c.rig.cell.is_observed(id));
    }
}

#[test]
fn observing_again_resends_the_current_value() {
    let c = counter();
    assert_eq!(c.rig.observe_on(0).len(), 1);
    assert_eq!(
        c.rig.observe_on(0).len(),
        1,
        "re-observe is a resync, not a no-op"
    );
    c.count.set(9);
    let again = c.rig.observe_on(0);
    assert_eq!(value_of::<i32>(&again[0]), 9);
}

#[test]
fn observe_off_writes_nothing_and_returns_zero() {
    let c = counter();
    c.rig.observe_all();
    let mut out = Writer::new();
    assert_eq!(c.rig.cell.observe(ALL_SIGNALS, false, &mut out), 0);
    assert!(out.is_empty());
}

#[test]
fn observe_all_on_an_empty_store_is_fine() {
    let cell = StoreCell::new(1);
    let mut out = Writer::new();
    assert_eq!(cell.observe(ALL_SIGNALS, true, &mut out), 0);
    assert!(out.is_empty());
}

#[test]
fn l2_observing_an_unknown_signal_is_ignored_in_every_build() {
    // The id comes from the host: it must never be able to make the core assert or panic.
    let c = counter();
    let mut out = Writer::new();
    assert_eq!(c.rig.cell.observe(9, true, &mut out), 0);
    assert!(out.is_empty());
    assert_eq!(c.rig.cell.observe(9, false, &mut out), 0);
    assert!(out.is_empty());
    assert!(!c.rig.cell.is_observed(9));
    // Nothing else was disturbed.
    assert_eq!(c.rig.cell.observe(0, true, &mut out), 1);
}

#[test]
fn observe_entries_use_the_change_set_entry_layout() {
    let cell = StoreCell::new(1);
    cell.set_handle(HANDLE);
    let s = Signal::new(5_u32);
    cell.attach(&s, 0).unwrap();
    let mut out = Writer::new();
    assert_eq!(cell.observe(0, true, &mut out), 1);
    assert_eq!(
        out.as_slice(),
        [
            7, 0, 0, 0, 1, 0, 0, 0, // handle
            0, 0, 0, 0, // signal_id
            0, // op = full
            4, 0, 0, 0, // len
            5, 0, 0, 0, // value
        ]
    );
}

// ---------------------------------------------------------------------------------------------
// commit
// ---------------------------------------------------------------------------------------------

#[test]
fn unobserved_writes_produce_no_change_set() {
    let c = counter();
    c.rig.run(|| {
        c.count.set(2);
        c.name.set("x".into());
        txn(|| c.count.set(3));
    });
    assert!(c.rig.sets().is_empty());
}

#[test]
fn an_observed_write_produces_one_change_set_with_the_full_value() {
    let c = counter();
    c.rig.observe_on(1);
    c.rig.run(|| c.name.set("ada".into()));
    let set = c.rig.one_set();
    assert_eq!(set.entries.len(), 1);
    let e = &set.entries[0];
    assert_eq!((e.handle, e.signal_id, e.op), (handle(), 1, ChangeOp::Full));
    assert_eq!(value_of::<String>(e), "ada");
}

#[test]
fn change_set_bytes_match_the_wire_layout() {
    let cell = StoreCell::new(1);
    cell.set_handle(HANDLE);
    let s = Signal::new(0_u32);
    cell.attach(&s, 0).unwrap();
    cell.observe(0, true, &mut Writer::new());
    let sink = CaptureSink::new();
    with_sink(sink.clone(), || s.set(5));
    let bytes = sink.take().remove(0);
    let txn_id = u64::from_le_bytes(bytes[..8].try_into().unwrap());
    assert!(txn_id >= 1);
    assert_eq!(
        &bytes[8..],
        [
            1, 0, 0, 0, // count
            7, 0, 0, 0, 1, 0, 0, 0, // handle
            0, 0, 0, 0, // signal_id
            0, // op = full
            4, 0, 0, 0, // len
            5, 0, 0, 0, // value
        ]
    );
}

#[test]
fn a_transaction_delivers_exactly_one_change_set_per_store() {
    let c = counter();
    c.rig.observe_all();
    c.rig.run(|| {
        txn(|| {
            c.count.set(2);
            c.name.set("x".into());
            c.count.set(3);
        });
    });
    let set = c.rig.one_set();
    assert_eq!(ids(&set), vec![0, 1, 2, 3]);
    assert_eq!(value_of::<i32>(entry(&set, 0)), 3);
    assert_eq!(value_of::<String>(entry(&set, 1)), "x");
    assert_eq!(value_of::<i32>(entry(&set, 2)), 6);
    assert_eq!(value_of::<String>(entry(&set, 3)), "x6");
}

#[test]
fn entries_are_ordered_by_signal_id_whatever_the_write_order() {
    let c = counter();
    c.rig.observe_all();
    c.rig.run(|| {
        txn(|| {
            c.name.set("b".into());
            c.count.set(7);
        });
    });
    assert_eq!(ids(&c.rig.one_set()), vec![0, 1, 2, 3]);
}

#[test]
fn a_signal_written_many_times_in_a_transaction_appears_once_with_the_last_value() {
    let c = counter();
    c.rig.observe_on(0);
    c.rig.run(|| {
        txn(|| {
            for v in 10..20 {
                c.count.set(v);
            }
        });
    });
    let set = c.rig.one_set();
    assert_eq!(set.entries.len(), 1);
    assert_eq!(value_of::<i32>(&set.entries[0]), 19);
}

#[test]
fn a_bare_set_is_an_implicit_transaction() {
    let c = counter();
    c.rig.observe_on(0);
    c.rig.run(|| {
        c.count.set(2);
        assert_eq!(c.rig.sink.len(), 1, "delivered before set returns");
        c.count.set(3);
        c.count.update(|v| *v += 1);
        assert_eq!(c.rig.sink.len(), 3, "one change-set per bare write");
    });
    let sets = c.rig.sets();
    let last = value_of::<i32>(&sets[2].entries[0]);
    assert_eq!(last, 4);
}

#[test]
fn nested_transactions_join_the_outer_one() {
    let c = counter();
    c.rig.observe_on(0);
    c.rig.run(|| {
        txn(|| {
            c.count.set(2);
            txn(|| {
                c.count.set(3);
                txn(|| c.count.set(4));
            });
            assert!(c.rig.sink.is_empty(), "inner transactions must not commit");
        });
    });
    let set = c.rig.one_set();
    assert_eq!(value_of::<i32>(&set.entries[0]), 4);
}

#[test]
fn nothing_is_delivered_before_the_outermost_transaction_ends() {
    let c = counter();
    c.rig.observe_on(0);
    c.rig.run(|| {
        txn(|| {
            c.count.set(2);
            assert!(c.rig.sink.is_empty());
        });
        assert_eq!(c.rig.sink.len(), 1);
    });
}

#[test]
fn a_transaction_without_writes_delivers_nothing() {
    let c = counter();
    c.rig.observe_all();
    c.rig.run(|| {
        txn(|| {});
        txn(|| {
            let _ = c.count.get();
        });
    });
    assert!(c.rig.sets().is_empty());
}

#[test]
fn values_are_encoded_at_commit_not_at_write() {
    let encodes = Arc::new(AtomicUsize::new(0));
    let rig = Rig::new();
    let s = Signal::new(Probe::new(0, &encodes));
    rig.cell.attach(&s, 0).unwrap();
    rig.observe_all();
    encodes.store(0, Ordering::SeqCst);
    rig.run(|| {
        txn(|| {
            for v in 1..=5 {
                s.set(Probe::new(v, &encodes));
            }
            assert_eq!(count(&encodes), 0, "no encoding inside the transaction");
        });
    });
    assert_eq!(count(&encodes), 1, "one encode per commit");
    assert_eq!(rig.one_set().entries.len(), 1);
}

#[test]
fn unobserved_writes_never_encode() {
    let encodes = Arc::new(AtomicUsize::new(0));
    let rig = Rig::new();
    let s = Signal::new(Probe::new(0, &encodes));
    rig.cell.attach(&s, 0).unwrap();
    rig.run(|| {
        for v in 0..1000 {
            s.set(Probe::new(v, &encodes));
        }
    });
    assert_eq!(count(&encodes), 0);
    assert!(rig.sets().is_empty());
}

#[test]
fn commit_touches_only_the_dirty_slots() {
    let encodes = Arc::new(AtomicUsize::new(0));
    let rig = Rig::new();
    let signals: Vec<Signal<Probe>> = (0..500)
        .map(|i| Signal::new(Probe::new(i, &encodes)))
        .collect();
    for (i, s) in signals.iter().enumerate() {
        rig.cell.attach(s, i as u32).unwrap();
    }
    assert_eq!(rig.observe_all().len(), 500);
    encodes.store(0, Ordering::SeqCst);
    rig.run(|| signals[123].set(Probe::new(999, &encodes)));
    assert_eq!(count(&encodes), 1, "only the written signal is encoded");
    let set = rig.one_set();
    assert_eq!(ids(&set), vec![123]);
}

#[test]
fn observing_after_unobserved_writes_sends_the_fresh_value() {
    let c = counter();
    c.rig.run(|| {
        c.count.set(10);
        c.count.set(20);
    });
    assert!(c.rig.sets().is_empty());
    let entries = c.rig.observe_on(0);
    assert_eq!(value_of::<i32>(&entries[0]), 20);
    // ...and the dirty state is clean: the next write is delivered normally.
    c.rig.run(|| c.count.set(21));
    assert_eq!(value_of::<i32>(&c.rig.one_set().entries[0]), 21);
}

#[test]
fn a_signal_that_was_never_written_still_sends_its_value_on_first_observe() {
    let c = counter();
    let entries = c.rig.observe_on(1);
    assert_eq!(value_of::<String>(&entries[0]), "n");
}

#[test]
fn writes_after_observe_off_and_on_again_are_delivered_once() {
    let c = counter();
    c.rig.observe_on(0);
    c.rig.observe_off(0);
    c.rig.run(|| c.count.set(2));
    c.rig.observe_on(0);
    c.rig.run(|| c.count.set(3));
    assert_eq!(c.rig.one_set().entries.len(), 1);
}

#[test]
fn commits_without_a_sink_are_harmless_and_do_not_wedge_the_slot() {
    let c = counter();
    c.rig.observe_on(0);
    // No sink is installed on this thread (and none globally in this test binary).
    c.count.set(2);
    c.count.set(3);
    // Installing a sink later delivers only new writes, and they do get delivered.
    c.rig.run(|| c.count.set(4));
    let set = c.rig.one_set();
    assert_eq!(value_of::<i32>(&set.entries[0]), 4);
}

#[test]
fn a_store_without_a_handle_consumes_writes_and_recovers_when_the_handle_arrives() {
    let cell = StoreCell::new(1);
    let s = Signal::new(0_u32);
    cell.attach(&s, 0).unwrap();
    let sink = CaptureSink::new();
    // Observed (say, by test code) but not yet published: nothing can be delivered.
    cell.observe(0, true, &mut Writer::new());
    with_sink(sink.clone(), || s.set(1));
    assert!(sink.is_empty());
    cell.set_handle(HANDLE);
    with_sink(sink.clone(), || s.set(2));
    let sets = sink.take_decoded();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].entries[0].handle, handle());
    assert_eq!(value_of::<u32>(&sets[0].entries[0]), 2);
}

#[test]
fn writes_before_attach_are_plain_writes() {
    let rig = Rig::new();
    let s = Signal::new(1_i32);
    rig.run(|| {
        s.set(2);
        s.set(3);
    });
    assert!(rig.sets().is_empty());
    rig.cell.attach(&s, 0).unwrap();
    let entries = rig.observe_on(0);
    assert_eq!(value_of::<i32>(&entries[0]), 3);
}

#[test]
fn a_dropped_store_does_not_break_its_signals() {
    let s = Signal::new(1_i32);
    let dependent = Computed::new(&s, |v| v + 1);
    {
        let cell = StoreCell::new(1);
        cell.set_handle(HANDLE);
        cell.attach(&s, 0).unwrap();
        cell.observe(0, true, &mut Writer::new());
    }
    let sink = CaptureSink::new();
    with_sink(sink.clone(), || s.set(5));
    assert!(sink.is_empty());
    assert_eq!(dependent.get(), 6);
}

// ---------------------------------------------------------------------------------------------
// computed signals
// ---------------------------------------------------------------------------------------------

#[test]
fn a_computed_is_delivered_only_when_observed() {
    let c = counter();
    c.rig.observe_on(0);
    c.rig.run(|| c.count.set(5));
    assert_eq!(ids(&c.rig.one_set()), vec![0]);
    c.rig.observe_on(2);
    c.rig.run(|| c.count.set(6));
    let set = c.rig.one_set();
    assert_eq!(ids(&set), vec![0, 2]);
    assert_eq!(value_of::<i32>(entry(&set, 2)), 12);
}

#[test]
fn an_observed_computed_is_recomputed_at_commit() {
    let c = counter();
    c.rig.observe_on(2);
    let before = count(&c.double_runs);
    c.rig.run(|| {
        txn(|| {
            c.count.set(2);
            c.count.set(3);
            assert_eq!(
                count(&c.double_runs),
                before,
                "not recomputed inside the txn"
            );
        });
    });
    assert_eq!(
        count(&c.double_runs),
        before + 1,
        "recomputed once, at commit"
    );
    assert_eq!(value_of::<i32>(&c.rig.one_set().entries[0]), 6);
    // Reading afterwards is a cache hit.
    assert_eq!(c.double.get(), 6);
    assert_eq!(count(&c.double_runs), before + 1);
}

#[test]
fn an_unobserved_computed_stays_lazy() {
    let c = counter();
    c.rig.observe_on(0);
    c.rig.run(|| c.count.set(2));
    c.rig.run(|| c.count.set(3));
    assert_eq!(
        count(&c.double_runs),
        0,
        "never observed, never read: never computed"
    );
    assert_eq!(c.double.get(), 6);
    assert_eq!(count(&c.double_runs), 1);
}

#[test]
fn a_computed_of_a_computed_carries_fresh_values() {
    let c = counter();
    c.rig.observe_on(3);
    c.rig.run(|| {
        c.name.set("z".into());
        c.count.set(4);
    });
    let sets = c.rig.sets();
    assert_eq!(sets.len(), 2, "two bare writes, two transactions");
    assert_eq!(value_of::<String>(&sets[0].entries[0]), "z2");
    assert_eq!(value_of::<String>(&sets[1].entries[0]), "z8");
}

#[test]
fn an_observed_computed_of_computeds_recomputes_each_node_once_per_commit() {
    let c = counter();
    c.rig.observe_all();
    let (d0, l0) = (count(&c.double_runs), count(&c.label_runs));
    c.rig.run(|| c.count.set(10));
    assert_eq!(count(&c.double_runs), d0 + 1);
    assert_eq!(count(&c.label_runs), l0 + 1);
    let set = c.rig.one_set();
    assert_eq!(value_of::<String>(entry(&set, 3)), "n20");
}

#[test]
fn a_diamond_computes_once_per_commit_even_when_every_node_is_observed() {
    let rig = Rig::new();
    let runs: Vec<Arc<AtomicUsize>> = (0..3).map(|_| Arc::new(AtomicUsize::new(0))).collect();
    let a = Signal::new(1_i32);
    let r = runs.clone();
    let left = Computed::new(&a, move |v: &i32| {
        r[0].fetch_add(1, Ordering::SeqCst);
        v + 1
    });
    let r = runs.clone();
    let right = Computed::new(&a, move |v: &i32| {
        r[1].fetch_add(1, Ordering::SeqCst);
        v * 10
    });
    let r = runs.clone();
    let sum = Computed::new((&left, &right), move |(l, rr): (&i32, &i32)| {
        r[2].fetch_add(1, Ordering::SeqCst);
        l + rr
    });
    rig.cell.attach(&a, 0).unwrap();
    rig.cell.attach_computed(&left, 1).unwrap();
    rig.cell.attach_computed(&right, 2).unwrap();
    rig.cell.attach_computed(&sum, 3).unwrap();
    rig.observe_all();
    let before: Vec<usize> = runs.iter().map(count).collect();
    rig.run(|| a.set(2));
    let after: Vec<usize> = runs.iter().map(count).collect();
    for i in 0..3 {
        assert_eq!(after[i], before[i] + 1, "node {i} computed once");
    }
    let set = rig.one_set();
    assert_eq!(ids(&set), vec![0, 1, 2, 3]);
    assert_eq!(value_of::<i32>(entry(&set, 3)), 3 + 20);
}

#[test]
fn a_computed_of_unattached_signals_is_delivered() {
    let rig = Rig::new();
    let loose = Signal::new(1_i32); // never attached to any store
    let derived = Computed::new(&loose, |v| v + 100);
    rig.cell.attach_computed(&derived, 0).unwrap();
    rig.observe_all();
    rig.run(|| loose.set(5));
    let set = rig.one_set();
    assert_eq!(value_of::<i32>(&set.entries[0]), 105);
}

#[test]
fn a_computed_fed_by_a_signal_of_another_store_is_delivered_by_its_own_store() {
    let one = Rig::new();
    let two = Rig::new();
    let src = Signal::new(1_i32);
    let derived = Computed::new(&src, |v| v * 3);
    one.cell.attach(&src, 0).unwrap();
    two.cell.attach_computed(&derived, 0).unwrap();
    one.observe_all();
    two.observe_all();
    with_sink(one.sink.clone(), || src.set(2));
    // Both stores committed on this thread, so both change-sets reached the thread's sink.
    let sets = one.sets();
    assert_eq!(sets.len(), 2);
    let derived_set = sets
        .iter()
        .find(|s| s.entries.iter().any(|e| value_of::<i32>(e) == 6))
        .expect("derived change-set");
    assert_eq!(derived_set.entries.len(), 1);
}

// ---------------------------------------------------------------------------------------------
// several stores and transaction ids
// ---------------------------------------------------------------------------------------------

#[test]
fn two_stores_in_one_transaction_share_a_txn_id() {
    let one = Rig::new();
    let two = Rig::new();
    two.cell.set_handle(0x0000_0002_0000_0009);
    let a = Signal::new(0_i32);
    let b = Signal::new(0_i32);
    one.cell.attach(&a, 0).unwrap();
    two.cell.attach(&b, 0).unwrap();
    one.observe_all();
    two.observe_all();
    let shared = CaptureSink::new();
    with_sink(shared.clone(), || {
        txn(|| {
            a.set(1);
            b.set(2);
        });
    });
    let sets = shared.take_decoded();
    assert_eq!(sets.len(), 2, "one change-set per store");
    assert_eq!(sets[0].txn_id, sets[1].txn_id);
    assert_ne!(sets[0].entries[0].handle, sets[1].entries[0].handle);
    assert_eq!(sets[0].entries[0].handle, handle());
    assert_eq!(sets[1].entries[0].handle.0, 0x0000_0002_0000_0009);
}

#[test]
fn stores_are_delivered_in_the_order_they_were_first_written() {
    let one = Rig::new();
    let two = Rig::new();
    two.cell.set_handle(0x0000_0002_0000_0009);
    let a = Signal::new(0_i32);
    let b = Signal::new(0_i32);
    one.cell.attach(&a, 0).unwrap();
    two.cell.attach(&b, 0).unwrap();
    one.observe_all();
    two.observe_all();
    let shared = CaptureSink::new();
    with_sink(shared.clone(), || {
        txn(|| {
            b.set(1);
            a.set(1);
            b.set(2);
        });
    });
    let sets = shared.take_decoded();
    assert_eq!(sets[0].entries[0].handle.0, 0x0000_0002_0000_0009);
    assert_eq!(sets[1].entries[0].handle, handle());
}

#[test]
fn separate_transactions_have_increasing_txn_ids() {
    let c = counter();
    c.rig.observe_on(0);
    c.rig.run(|| {
        c.count.set(2);
        c.count.set(3);
        txn(|| c.count.set(4));
    });
    let sets = c.rig.sets();
    assert_eq!(sets.len(), 3);
    assert!(sets[0].txn_id < sets[1].txn_id && sets[1].txn_id < sets[2].txn_id);
    assert!(next_txn_id() > sets[2].txn_id);
}

#[test]
fn a_store_that_is_not_dirty_is_not_part_of_the_transaction() {
    let one = Rig::new();
    let two = Rig::new();
    let a = Signal::new(0_i32);
    let b = Signal::new(0_i32);
    one.cell.attach(&a, 0).unwrap();
    two.cell.attach(&b, 0).unwrap();
    one.observe_all();
    two.observe_all();
    let shared = CaptureSink::new();
    with_sink(shared.clone(), || a.set(1));
    assert_eq!(shared.take_decoded().len(), 1);
}

// ---------------------------------------------------------------------------------------------
// no_coalesce
// ---------------------------------------------------------------------------------------------

#[test]
fn no_coalesce_signals_are_delivered_even_while_unobserved() {
    let c = counter();
    c.rig.cell.set_no_coalesce(1).unwrap();
    c.rig.run(|| {
        c.name.set("a".into());
        c.name.set("b".into());
        c.count.set(5); // ordinary, unobserved: not delivered
    });
    let sets = c.rig.sets();
    assert_eq!(
        sets.len(),
        2,
        "the ordinary unobserved write delivers nothing"
    );
    assert_eq!(sets[0].entries.len(), 1);
    assert_eq!(value_of::<String>(&sets[0].entries[0]), "a");
    assert_eq!(value_of::<String>(&sets[1].entries[0]), "b");
}

#[test]
fn no_coalesce_shares_the_change_set_of_the_observed_signals() {
    let c = counter();
    c.rig.cell.set_no_coalesce(1).unwrap();
    c.rig.observe_on(0);
    c.rig.run(|| {
        txn(|| {
            c.count.set(2);
            c.name.set("q".into());
        });
    });
    assert_eq!(ids(&c.rig.one_set()), vec![0, 1]);
}

// ---------------------------------------------------------------------------------------------
// encode_signal and snapshots
// ---------------------------------------------------------------------------------------------

#[test]
fn encode_signal_writes_the_full_value_and_reports_unknown_ids() {
    let c = counter();
    c.count.set(7);
    let mut w = Writer::new();
    assert!(c.rig.cell.encode_signal(0, &mut w));
    assert_eq!(w.as_slice(), 7_i32.to_le_bytes());
    let mut w = Writer::new();
    assert!(c.rig.cell.encode_signal(1, &mut w));
    assert_eq!(w.as_slice(), [1, 0, 0, 0, b'n']);
    let mut w = Writer::new();
    assert!(c.rig.cell.encode_signal(2, &mut w), "computeds encode too");
    assert_eq!(w.as_slice(), 14_i32.to_le_bytes());
    let mut w = Writer::new();
    assert!(!c.rig.cell.encode_signal(4, &mut w));
    assert!(!c.rig.cell.encode_signal(ALL_SIGNALS, &mut w));
    assert!(w.is_empty(), "nothing is written for an unknown id");
}

#[test]
fn encode_signal_does_not_change_observation_or_dirtiness() {
    let c = counter();
    let mut w = Writer::new();
    assert!(c.rig.cell.encode_signal(0, &mut w));
    assert!(!c.rig.cell.is_observed(0));
    c.rig.observe_on(0);
    c.rig.run(|| c.count.set(3));
    let mut w = Writer::new();
    c.rig.cell.encode_signal(0, &mut w);
    assert_eq!(c.rig.one_set().entries.len(), 1);
}

#[test]
fn the_snapshot_excludes_computeds_and_round_trips() {
    let c = counter();
    c.count.set(9);
    c.name.set("snap".into());
    let mut w = Writer::new();
    c.rig.cell.encode_snapshot(&mut w);
    let mut r = Reader::new(w.as_slice());
    let snap = StoreSnapshot::decode(&mut r).expect("decodes as a store snapshot");
    r.finish().expect("no trailing bytes");
    assert_eq!(snap.handle, handle());
    assert_eq!(snap.type_id, 0xF00D);
    let signal_ids: Vec<u32> = snap.signals.iter().map(|(id, _)| *id).collect();
    assert_eq!(
        signal_ids,
        vec![0, 1],
        "computeds (2, 3) are recomputed on restore"
    );
    assert_eq!(snap.signals[0].1, 9_i32.to_le_bytes());
    assert_eq!(snap.signals[1].1, [4, 0, 0, 0, b's', b'n', b'a', b'p']);
}

#[test]
fn snapshot_of_an_empty_store_has_no_signals() {
    let cell = StoreCell::new(3);
    cell.set_handle(5);
    let mut w = Writer::new();
    cell.encode_snapshot(&mut w);
    let snap = StoreSnapshot::decode(&mut Reader::new(w.as_slice())).unwrap();
    assert_eq!(snap.type_id, 3);
    assert_eq!(snap.handle.0, 5);
    assert!(snap.signals.is_empty());
}

#[test]
fn snapshot_does_not_disturb_delivery() {
    let c = counter();
    c.rig.observe_on(0);
    c.rig.run(|| c.count.set(2));
    let mut w = Writer::new();
    c.rig.cell.encode_snapshot(&mut w);
    assert_eq!(c.rig.one_set().entries.len(), 1);
}
