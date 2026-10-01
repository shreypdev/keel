//! The process-wide sink and writes from several threads.
//!
//! These tests install the global sink, so they serialise on a lock; every other test binary
//! uses thread-scoped sinks and is free to run in parallel.

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use parking_lot::Mutex;
use undra_signals::testing::{CaptureSink, decode};
use undra_signals::{
    ChangeSink, Computed, Signal, StoreCell, clear_sink, set_sink, txn, with_sink,
};
use undra_wire::Writer;

static SERIAL: Mutex<()> = Mutex::new(());

/// Holds the serialisation lock and clears the global sink when the test ends, even by panic.
struct Exclusive {
    _guard: parking_lot::MutexGuard<'static, ()>,
}

fn exclusive() -> Exclusive {
    let guard = SERIAL.lock();
    clear_sink();
    Exclusive { _guard: guard }
}

impl Drop for Exclusive {
    fn drop(&mut self) {
        clear_sink();
    }
}

/// Checks that every payload decodes and remembers the newest `u32` seen per signal.
#[derive(Default)]
struct Validating {
    sets: AtomicUsize,
    entries: AtomicUsize,
    seen: Mutex<HashMap<u32, Vec<u32>>>,
}

impl ChangeSink for Validating {
    fn deliver(&self, change_set: &[u8]) {
        let set = decode(change_set).expect("every change-set must decode");
        assert!(
            !set.entries.is_empty(),
            "empty change-sets are never delivered"
        );
        self.sets.fetch_add(1, Ordering::SeqCst);
        self.entries.fetch_add(set.entries.len(), Ordering::SeqCst);
        let mut seen = self.seen.lock();
        for e in &set.entries {
            assert_eq!(e.handle, handle());
            seen.entry(e.signal_id)
                .or_default()
                .push(value_of::<u32>(e));
        }
    }
}

fn store_with(n: u32) -> (Arc<StoreCell>, Vec<Signal<u32>>) {
    let cell = StoreCell::new(0xBEEF);
    cell.set_handle(HANDLE);
    let signals: Vec<Signal<u32>> = (0..n).map(|_| Signal::new(0)).collect();
    for (i, s) in signals.iter().enumerate() {
        cell.attach(s, u32::try_from(i).unwrap()).unwrap();
    }
    cell.observe(undra_signals::ALL_SIGNALS, true, &mut Writer::new());
    (cell, signals)
}

#[test]
fn the_global_sink_receives_commits_made_on_any_thread() {
    let _x = exclusive();
    let (_cell, signals) = store_with(1);
    let capture = CaptureSink::new();
    set_sink(capture.clone());
    let s = signals[0].clone();
    std::thread::spawn(move || s.set(11)).join().unwrap();
    signals[0].set(12);
    let sets = capture.take_decoded();
    assert_eq!(sets.len(), 2);
    assert_eq!(value_of::<u32>(&sets[0].entries[0]), 11);
    assert_eq!(value_of::<u32>(&sets[1].entries[0]), 12);
}

#[test]
fn set_sink_replaces_the_previous_sink() {
    let _x = exclusive();
    let (_cell, signals) = store_with(1);
    let first = CaptureSink::new();
    let second = CaptureSink::new();
    set_sink(first.clone());
    signals[0].set(1);
    set_sink(second.clone());
    signals[0].set(2);
    assert_eq!(first.take_decoded().len(), 1);
    assert_eq!(second.take_decoded().len(), 1);
}

#[test]
fn clear_sink_stops_delivery() {
    let _x = exclusive();
    let (_cell, signals) = store_with(1);
    let capture = CaptureSink::new();
    set_sink(capture.clone());
    signals[0].set(1);
    clear_sink();
    signals[0].set(2);
    assert_eq!(capture.take_decoded().len(), 1);
}

#[test]
fn a_thread_scoped_sink_takes_precedence_over_the_global_one() {
    let _x = exclusive();
    let (_cell, signals) = store_with(1);
    let global = CaptureSink::new();
    let local = CaptureSink::new();
    set_sink(global.clone());
    with_sink(local.clone(), || signals[0].set(1));
    signals[0].set(2);
    assert_eq!(local.take_decoded().len(), 1);
    assert_eq!(global.take_decoded().len(), 1);
}

#[test]
fn a_sink_can_replace_itself_from_inside_deliver() {
    let _x = exclusive();
    let (_cell, signals) = store_with(1);
    let second = CaptureSink::new();
    struct Swap(Arc<CaptureSink>);
    impl ChangeSink for Swap {
        fn deliver(&self, _change_set: &[u8]) {
            set_sink(self.0.clone());
        }
    }
    set_sink(Arc::new(Swap(second.clone())));
    signals[0].set(1);
    signals[0].set(2);
    assert_eq!(
        second.take_decoded().len(),
        1,
        "only the second write reaches the new sink"
    );
}

#[test]
fn four_threads_writing_different_signals_of_one_store() {
    let _x = exclusive();
    const WRITES: u32 = 500;
    let (_cell, signals) = store_with(4);
    let sink = Arc::new(Validating::default());
    set_sink(sink.clone());

    std::thread::scope(|scope| {
        for s in &signals {
            scope.spawn(move || {
                for v in 1..=WRITES {
                    s.set(v);
                }
            });
        }
    });

    assert_eq!(
        sink.sets.load(Ordering::SeqCst),
        4 * WRITES as usize,
        "one change-set per write"
    );
    assert_eq!(sink.entries.load(Ordering::SeqCst), 4 * WRITES as usize);
    let seen = sink.seen.lock();
    for id in 0..4 {
        let values = &seen[&id];
        assert_eq!(values.len(), WRITES as usize);
        assert!(
            values.windows(2).all(|w| w[0] < w[1]),
            "signal {id} delivered out of order"
        );
        assert_eq!(*values.last().unwrap(), WRITES);
    }
}

#[test]
fn four_threads_each_committing_two_signals_per_transaction() {
    let _x = exclusive();
    const ROUNDS: u32 = 300;
    let (_cell, signals) = store_with(8);
    let sink = Arc::new(Validating::default());
    set_sink(sink.clone());

    std::thread::scope(|scope| {
        for pair in signals.chunks(2) {
            scope.spawn(move || {
                for v in 1..=ROUNDS {
                    txn(|| {
                        pair[0].set(v);
                        pair[1].set(v);
                    });
                }
            });
        }
    });

    assert_eq!(sink.sets.load(Ordering::SeqCst), 4 * ROUNDS as usize);
    assert_eq!(
        sink.entries.load(Ordering::SeqCst),
        8 * ROUNDS as usize,
        "each transaction is one change-set with both of its entries"
    );
}

#[test]
fn threads_hammering_the_same_signal_deliver_only_valid_change_sets() {
    let _x = exclusive();
    let (_cell, signals) = store_with(1);
    let sink = Arc::new(Validating::default());
    set_sink(sink.clone());
    let s = signals[0].clone();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let s = s.clone();
            scope.spawn(move || {
                for _ in 0..500 {
                    s.update(|v| *v += 1);
                }
            });
        }
    });
    assert_eq!(s.get(), 2000, "no update is lost");
    let sets = sink.sets.load(Ordering::SeqCst);
    assert!((1..=2000).contains(&sets), "{sets} change-sets");
    // Concurrent commits of one slot may coalesce (one commit delivers the value that several
    // writes produced), but never invent values.
    assert!(sink.seen.lock()[&0].iter().all(|v| (1..=2000).contains(v)));
}

#[test]
fn observing_while_another_thread_writes() {
    let _x = exclusive();
    let (cell, signals) = store_with(2);
    let sink = Arc::new(Validating::default());
    set_sink(sink.clone());
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    std::thread::scope(|scope| {
        let flag = done.clone();
        let cell2 = cell.clone();
        scope.spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                let mut sink = Writer::new();
                cell2.observe(0, false, &mut sink);
                cell2.observe(0, true, &mut sink);
                cell2.observe(undra_signals::ALL_SIGNALS, true, &mut sink);
            }
        });
        let writer_signals = signals.clone();
        let flag = done.clone();
        scope.spawn(move || {
            for v in 1..=2000 {
                writer_signals[0].set(v);
                writer_signals[1].set(v);
            }
            flag.store(true, Ordering::SeqCst);
        });
    });
    assert!(sink.sets.load(Ordering::SeqCst) >= 1);
}

#[test]
fn a_computed_over_signals_written_by_several_threads_converges() {
    let _x = exclusive();
    let (cell, signals) = store_with(4);
    let total = Computed::new(
        (&signals[0], &signals[1], &signals[2], &signals[3]),
        |(a, b, c, d)| a + b + c + d,
    );
    cell.attach_computed(&total, 4).unwrap();
    cell.observe(4, true, &mut Writer::new());
    let sink = Arc::new(Validating::default());
    set_sink(sink.clone());
    std::thread::scope(|scope| {
        for s in &signals {
            scope.spawn(move || {
                for v in 1..=200 {
                    s.set(v);
                }
            });
        }
    });
    assert_eq!(total.get(), 800);
    assert!(
        sink.seen.lock().contains_key(&4),
        "the computed was delivered"
    );
}

// ---------------------------------------------------------------------------------------------
// Recorded list operations from several threads (ADR-027)
// ---------------------------------------------------------------------------------------------

/// A host for one keyed list: applies every change-set it is handed, failing on a patch that does
/// not apply to what it has. It is the sink, so it runs under the store's delivery lock.
struct ListHost {
    list: Mutex<Vec<Todo>>,
    patches: AtomicUsize,
    fulls: AtomicUsize,
}

impl ListHost {
    fn new() -> Arc<ListHost> {
        Arc::new(ListHost {
            list: Mutex::new(Vec::new()),
            patches: AtomicUsize::new(0),
            fulls: AtomicUsize::new(0),
        })
    }
}

impl ChangeSink for ListHost {
    fn deliver(&self, change_set: &[u8]) {
        use undra_wire::payload::ChangeOp;
        let set = decode(change_set).expect("every change-set must decode");
        let mut list = self.list.lock();
        for e in &set.entries {
            match e.op {
                ChangeOp::Full => {
                    *list = value_of(e);
                    self.fulls.fetch_add(1, Ordering::SeqCst);
                }
                ChangeOp::KeyedPatch => {
                    patch_of::<Todo>(e)
                        .apply(&mut list)
                        .expect("a delivered patch applies to what the host has");
                    self.patches.fetch_add(1, Ordering::SeqCst);
                }
                other => panic!("unexpected op {other:?}"),
            }
        }
    }
}

/// A list of 60 items at signal 0 of a fresh store, observed, with the host holding it.
fn list_store() -> (Arc<StoreCell>, Signal<Vec<Todo>>, Arc<ListHost>) {
    let cell = StoreCell::new(0xBEE5);
    cell.set_handle(HANDLE);
    let list = Signal::new(todos(60));
    cell.attach_keyed(&list, 0, todo_key).unwrap();
    let host = ListHost::new();
    cell.observe_and_deliver(&[0], |payload| host.deliver(payload));
    (cell, list, host)
}

/// Rounds of `hammer` per thread: `UNDRA_SIGNALS_STRESS_ROUNDS` for a longer soak, 400 otherwise.
fn stress_rounds() -> u32 {
    std::env::var("UNDRA_SIGNALS_STRESS_ROUNDS")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(400)
}

/// One thread's share of the work: recorded operations of every kind, a raw write and a
/// multi-operation transaction now and then. The list never drops below 12 items (at most 40
/// removals in total against 60 to start with), so the fixed indices are always in range.
fn hammer(list: &Signal<Vec<Todo>>, thread: u32, rounds: u32) {
    let mut removals = 0;
    for n in 0..rounds {
        let id = 1_000 + thread * 1_000_000 + n;
        match n % 8 {
            0 => list.push(todo(id, "p", false)),
            1 => list.insert(0, todo(id, "i", true)),
            2 => list.update_at((n % 9) as usize, |t| t.done = !t.done),
            3 => list.move_item(1, 10),
            4 if removals < 10 => {
                removals += 1;
                drop(list.remove((n % 9) as usize));
            }
            4 => list.push(todo(id, "p", false)),
            5 => list.update(|l| l[2].title.push('r')),
            6 => txn(|| {
                list.push(todo(id, "t", false));
                list.move_item(0, 3);
                list.update_at(1, |t| t.title.push('x'));
            }),
            _ => list.move_item(11, 2),
        }
    }
}

#[test]
fn recorded_operations_from_four_threads_replay_to_the_cores_list() {
    let _x = exclusive();
    let (_cell, list, host) = list_store();
    set_sink(host.clone());
    std::thread::scope(|scope| {
        for thread in 0..4 {
            let list = list.clone();
            scope.spawn(move || hammer(&list, thread, stress_rounds()));
        }
    });
    assert_eq!(
        *host.list.lock(),
        list.get(),
        "every op some commit took was sent, in order, exactly once"
    );
    assert!(host.patches.load(Ordering::SeqCst) > 0);
    assert_eq!(
        host.fulls.load(Ordering::SeqCst),
        1,
        "only the initial observe sent the list in full: the rest were patches"
    );
}

#[test]
fn recorded_operations_while_another_thread_observes_and_unobserves() {
    let _x = exclusive();
    let (cell, list, host) = list_store();
    set_sink(host.clone());
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    std::thread::scope(|scope| {
        let (flag, cell) = (done.clone(), cell.clone());
        let observer_host = host.clone();
        scope.spawn(move || {
            let mut round = 0_u32;
            while !flag.load(Ordering::SeqCst) {
                round += 1;
                if round % 3 == 0 {
                    cell.observe(0, false, &mut Writer::new());
                }
                // Observing is a resynchronisation: the host gets the full value, under the
                // delivery lock, so no commit can put an older list after it.
                cell.observe_and_deliver(&[0], |payload| observer_host.deliver(payload));
            }
        });
        let writers: Vec<_> = (0..3)
            .map(|thread| {
                let list = list.clone();
                scope.spawn(move || hammer(&list, thread, stress_rounds()))
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        done.store(true, Ordering::SeqCst);
    });

    // Whatever the interleaving was, the core and the host are at a state from which patches
    // keep working: resynchronise once more, write some more, compare.
    cell.observe_and_deliver(&[0], |payload| host.deliver(payload));
    assert_eq!(*host.list.lock(), list.get());
    let patches_before = host.patches.load(Ordering::SeqCst);
    hammer(&list, 9, 64);
    assert_eq!(*host.list.lock(), list.get());
    assert!(
        host.patches.load(Ordering::SeqCst) > patches_before,
        "and the writes after the chaos are patches again"
    );
}
