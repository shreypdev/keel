//! Observation, change-set delivery, release, and snapshot/restore.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use keel_meta::ids;
use keel_meta::ids::ALL_SIGNALS;
use keel_runtime::testing::{HostEvent, TestRuntime};
use keel_runtime::{Handle, RestoreError, Runtime, RuntimeConfig};
use keel_wire::payload::{ChangeEntry, ChangeOp, ChangeSet, ReplyStatus, Snapshot, StoreSnapshot};
use keel_wire::{Reader, WireError, Writer};

fn entry(handle: Handle, signal_id: u32, value: Vec<u8>) -> ChangeEntry {
    ChangeEntry {
        handle,
        signal_id,
        op: ChangeOp::Full,
        value,
    }
}

fn single(t: &TestRuntime) -> ChangeSet {
    let mut sets = t.host().take_decoded_change_sets();
    assert_eq!(sets.len(), 1, "expected exactly one change-set: {sets:?}");
    sets.remove(0)
}

fn get(t: &TestRuntime, handle: Handle) -> i32 {
    decode_body(&call_counter(t, handle, GET, 900, &[]))
}

// ----- observe ----------------------------------------------------------------------------

#[test]
fn observe_delivers_the_initial_change_set_before_returning() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 5, "five");
    assert_eq!(t.host().change_set_count(), 0, "nothing is observed yet");

    t.runtime().observe(h.0, ALL_SIGNALS, true);
    // Synchronous: no run_pending, no waiting.
    let cs = single(&t);
    assert_eq!(
        cs.entries,
        [
            entry(h, COUNT_SIGNAL, enc(&5_i32)),
            entry(h, LABEL_SIGNAL, enc("five"))
        ]
    );
    assert!(cs.txn_id > 0);
}

#[test]
fn observing_one_signal_delivers_only_that_signal() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 5, "five");
    t.runtime().observe(h.0, LABEL_SIGNAL, true);
    assert_eq!(single(&t).entries, [entry(h, LABEL_SIGNAL, enc("five"))]);
}

/// Review finding L2: an unknown signal id is host input, so it must not assert or poison.
#[test]
fn l2_observing_an_unknown_signal_id_is_ignored_quietly() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 5, "five");
    t.host().take_logs();
    t.runtime().observe(h.0, 99, true);
    t.runtime().observe(h.0, 99, false);
    assert_eq!(t.host().change_set_count(), 0);
    assert!(
        t.host().take_logs().iter().all(|l| l.level < 3),
        "no warning or error is logged for it"
    );
    // The store is healthy: it is not poisoned and a snapshot/restore keeps working.
    assert_eq!(get(&t, h), 5);
    let snapshot = t.runtime().snapshot();
    t.runtime().restore(&snapshot).unwrap();
    assert_eq!(get(&t, h), 5);
}

#[test]
fn unobserving_delivers_nothing_and_stops_further_writes() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 0, "");
    t.runtime().observe(h.0, ALL_SIGNALS, true);
    t.host().take_change_sets();
    t.runtime().observe(h.0, ALL_SIGNALS, false);
    assert_eq!(t.host().change_set_count(), 0);
    call_counter(&t, h, ADD, 2, &enc(&1_i32));
    assert_eq!(t.host().change_set_count(), 0);
}

#[test]
fn observe_of_bad_handles_and_non_stores_is_logged_and_ignored() {
    let t = TestRuntime::new();
    t.runtime().observe(0, ALL_SIGNALS, true);
    t.runtime().observe(Handle::new(9, 9).0, ALL_SIGNALS, true);
    let lazy = Handle(decode_body::<u64>(&t.call_sync(
        function_target(MAKE_LAZY),
        1,
        &enc(&1_u32),
    )));
    t.runtime().observe(lazy.0, ALL_SIGNALS, true);
    assert_eq!(t.host().change_set_count(), 0);
    let logs = t.host().take_logs();
    assert_eq!(logs.iter().filter(|l| l.level == 3).count(), 3, "{logs:?}");
    assert!(logs.iter().any(|l| l.message.contains("not a store")));
}

// ----- writes -----------------------------------------------------------------------------

#[test]
fn a_store_write_inside_a_method_reaches_the_host_as_a_change_set() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 5, "five");
    t.runtime().observe(h.0, ALL_SIGNALS, true);
    t.host().take_change_sets();

    let added: i32 = decode_body(&call_counter(&t, h, ADD, 2, &enc(&3_i32)));
    assert_eq!(added, 8);
    assert_eq!(single(&t).entries, [entry(h, COUNT_SIGNAL, enc(&8_i32))]);
}

#[test]
fn only_observed_signals_are_delivered() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 0, "");
    t.runtime().observe(h.0, COUNT_SIGNAL, true);
    t.host().take_change_sets();
    // add_twice writes both signals in one transaction; only the observed one is delivered.
    call_counter(&t, h, ADD_TWICE, 2, &enc(&4_i32));
    assert_eq!(single(&t).entries, [entry(h, COUNT_SIGNAL, enc(&4_i32))]);
}

#[test]
fn a_transaction_is_one_change_set_and_txn_ids_increase() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 0, "");
    t.runtime().observe(h.0, ALL_SIGNALS, true);
    let initial = single(&t);

    call_counter(&t, h, ADD_TWICE, 2, &enc(&2_i32));
    let first = single(&t);
    assert_eq!(
        first.entries,
        [
            entry(h, COUNT_SIGNAL, enc(&2_i32)),
            entry(h, LABEL_SIGNAL, enc("n=2"))
        ]
    );
    call_counter(&t, h, ADD, 3, &enc(&1_i32));
    let second = single(&t);
    assert!(initial.txn_id < first.txn_id && first.txn_id < second.txn_id);
}

#[test]
fn unobserved_writes_are_silent_and_observe_reports_the_current_value() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 0, "");
    call_counter(&t, h, ADD, 2, &enc(&41_i32));
    call_counter(&t, h, ADD, 3, &enc(&1_i32));
    assert_eq!(t.host().change_set_count(), 0);
    t.runtime().observe(h.0, COUNT_SIGNAL, true);
    assert_eq!(single(&t).entries, [entry(h, COUNT_SIGNAL, enc(&42_i32))]);
}

#[test]
fn writes_from_async_tasks_arrive_before_the_reply_they_belong_to() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 1, "");
    t.runtime().observe(h.0, COUNT_SIGNAL, true);
    t.host().take_timeline();
    assert_eq!(t.call(counter_target(h, SLOW_ADD), 5, &enc(&2_i32)), 0);
    t.run_pending();
    t.advance(Duration::from_millis(10));
    assert_eq!(
        t.host().take_timeline(),
        [HostEvent::ChangeSet, HostEvent::Reply(5)],
        "commit order: the change-set precedes the reply"
    );
}

#[test]
fn writes_from_detached_tasks_are_delivered_too() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 1, "");
    t.runtime().observe(h.0, COUNT_SIGNAL, true);
    t.host().take_change_sets();
    call_counter(&t, h, SPAWN_LATER, 2, &[]);
    assert_eq!(t.host().change_set_count(), 0);
    t.run_pending();
    assert_eq!(single(&t).entries, [entry(h, COUNT_SIGNAL, enc(&101_i32))]);
}

#[test]
fn change_sets_go_to_the_runtime_that_owns_the_store() {
    let a = TestRuntime::new();
    let b = TestRuntime::new();
    let ha = new_counter(&a, 0, "");
    let hb = new_counter(&b, 0, "");
    a.runtime().observe(ha.0, ALL_SIGNALS, true);
    b.runtime().observe(hb.0, ALL_SIGNALS, true);
    a.host().take_change_sets();
    b.host().take_change_sets();

    call_counter(&a, ha, ADD, 2, &enc(&1_i32));
    assert_eq!(a.host().change_set_count(), 1);
    assert_eq!(b.host().change_set_count(), 0);
    call_counter(&b, hb, ADD, 2, &enc(&1_i32));
    assert_eq!(b.host().change_set_count(), 1);
    assert_eq!(a.host().change_set_count(), 1);
}

#[test]
fn ctx_txn_from_outside_a_call_routes_through_that_runtime() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 0, "");
    t.runtime().observe(h.0, ALL_SIGNALS, true);
    t.host().take_change_sets();
    let counter = t.runtime().object::<Counter>(h.0).unwrap();
    t.ctx().txn(|| {
        counter.count.set(7);
        counter.label.set("seven".to_owned());
    });
    assert_eq!(
        single(&t).entries,
        [
            entry(h, COUNT_SIGNAL, enc(&7_i32)),
            entry(h, LABEL_SIGNAL, enc("seven"))
        ]
    );
}

#[test]
fn a_panic_after_a_write_still_delivers_the_write() {
    // The write happened (values are consistent per write), so observers must hear about it.
    let t = TestRuntime::new();
    let h = new_counter(&t, 0, "");
    t.runtime().observe(h.0, COUNT_SIGNAL, true);
    t.host().take_change_sets();
    let counter = t.runtime().object::<Counter>(h.0).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        t.ctx().txn(|| {
            counter.count.set(3);
            panic!("mid-transaction");
        })
    }));
    assert!(result.is_err());
    assert_eq!(single(&t).entries, [entry(h, COUNT_SIGNAL, enc(&3_i32))]);
}

// ----- release ----------------------------------------------------------------------------

#[test]
fn release_invalidates_the_handle_and_detaches_the_store() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 0, "");
    t.runtime().observe(h.0, ALL_SIGNALS, true);
    t.host().take_change_sets();
    let survivor = t.runtime().object::<Counter>(h.0).unwrap();

    t.runtime().release(h.0);
    assert_eq!(t.runtime().objects().live(), 0);
    assert_eq!(
        call_counter(&t, h, GET, 2, &[]).status,
        ReplyStatus::BadRequest
    );
    // A task that still holds the object can keep using it, but it no longer talks to the host.
    survivor.count.set(99);
    assert_eq!(t.host().change_set_count(), 0);
    assert_eq!(survivor.count.get(), 99);
    // Releasing again, or observing the stale handle, is harmless.
    t.runtime().release(h.0);
    t.runtime().observe(h.0, ALL_SIGNALS, true);
    assert_eq!(t.host().change_set_count(), 0);
}

/// Review finding M3: a commit cut off at `keel-signals`' round cap is reported as an error
/// through the runtime's log, and it leaves the store usable.
#[test]
fn m3_a_commit_cut_off_at_the_round_cap_is_logged_as_an_error() {
    use keel_signals::Effect;
    let t = TestRuntime::new();
    let h = new_counter(&t, 0, "");
    t.runtime().observe(h.0, ALL_SIGNALS, true);
    t.host().take_change_sets();
    t.host().take_logs();
    let counter = t.runtime().object::<Counter>(h.0).unwrap();

    // Two effects that keep waking each other: count -> label -> count -> ...
    let label = counter.label.clone();
    let to_label = Effect::new(&counter.count, move |n: &i32| label.set(n.to_string()));
    let count = counter.count.clone();
    let to_count = Effect::new(&counter.label, move |l: &String| count.set(l.len() as i32));
    {
        let _scope = t.ctx().enter();
        counter.count.set(12);
    }
    let logs = t.host().take_logs();
    let capped: Vec<_> = logs
        .iter()
        .filter(|l| l.message.contains("effect loop hit the round cap"))
        .collect();
    assert_eq!(capped.len(), 1, "{logs:?}");
    assert_eq!(capped[0].level, 4, "error level");
    assert_eq!(capped[0].target, "keel::signals");

    // The store still delivers: the last change-set carries the values the core ended with, and
    // the next write reaches the host.
    let sets = t.host().take_decoded_change_sets();
    assert!(!sets.is_empty());
    drop((to_label, to_count));
    t.host().take_change_sets();
    {
        let _scope = t.ctx().enter();
        counter.count.set(5);
    }
    assert_eq!(single(&t).entries, [entry(h, COUNT_SIGNAL, enc(&5_i32))]);
}

#[test]
fn a_reused_slot_gets_a_fresh_generation_end_to_end() {
    let t = TestRuntime::new();
    let first = new_counter(&t, 1, "");
    t.runtime().release(first.0);
    let second = new_counter(&t, 2, "");
    assert_eq!(first.index(), second.index());
    assert_eq!(second.generation(), first.generation() + 1);
    assert_eq!(get(&t, second), 2);
    assert_eq!(
        call_counter(&t, first, GET, 5, &[]).status,
        ReplyStatus::BadRequest
    );
}

// ----- dev mode ---------------------------------------------------------------------------

#[test]
fn dev_mode_logs_every_commit() {
    let t = TestRuntime::with_config(RuntimeConfig {
        mode: "dev".into(),
        log_level: 0,
        ..RuntimeConfig::default()
    });
    let h = new_counter(&t, 0, "");
    t.runtime().observe(h.0, COUNT_SIGNAL, true);
    call_counter(&t, h, ADD, 2, &enc(&1_i32));
    let logs = t.host().take_logs();
    let commits: Vec<_> = logs
        .iter()
        .filter(|l| l.target == "keel::devtools" && l.message.starts_with("commit txn="))
        .collect();
    assert_eq!(commits.len(), 2, "the observe and the write: {logs:?}");
    assert!(commits[1].message.contains("entries=1"));
}

// ----- snapshot ---------------------------------------------------------------------------

fn snapshot_of(t: &TestRuntime) -> Snapshot {
    let bytes = t.runtime().snapshot();
    let mut r = Reader::new(&bytes);
    let snap = Snapshot::decode(&mut r).expect("a snapshot is a wire Snapshot");
    r.finish().expect("no trailing bytes");
    snap
}

#[test]
fn snapshot_is_a_wire_snapshot_of_the_stores_only() {
    let t = TestRuntime::new();
    let h1 = new_counter(&t, 5, "a");
    let h2 = new_counter(&t, 7, "b");
    let lazy = Handle(decode_body::<u64>(&t.call_sync(
        function_target(MAKE_LAZY),
        1,
        &enc(&2_u32),
    )));
    assert_ne!(lazy, h1);

    let snap = snapshot_of(&t);
    assert_eq!(
        snap.stores,
        [
            StoreSnapshot {
                handle: h1,
                type_id: ids::type_id("Counter"),
                signals: vec![(COUNT_SIGNAL, enc(&5_i32)), (LABEL_SIGNAL, enc("a"))],
            },
            StoreSnapshot {
                handle: h2,
                type_id: ids::type_id("Counter"),
                signals: vec![(COUNT_SIGNAL, enc(&7_i32)), (LABEL_SIGNAL, enc("b"))],
            },
        ]
    );
}

#[test]
fn snapshot_of_an_empty_runtime_is_an_empty_snapshot() {
    let t = TestRuntime::new();
    assert_eq!(t.runtime().snapshot(), [0, 0, 0, 0]);
}

// ----- restore ----------------------------------------------------------------------------

#[test]
fn restore_round_trip_keeps_handles_and_reemits_observed_signals() {
    let t = TestRuntime::new();
    let h1 = new_counter(&t, 5, "a");
    let h2 = new_counter(&t, 7, "b");
    let lazy = Handle(decode_body::<u64>(&t.call_sync(
        function_target(MAKE_LAZY),
        1,
        &enc(&2_u32),
    )));
    call_counter(&t, h1, ADD, 2, &enc(&1_i32)); // h1 = 6
    t.runtime().observe(h1.0, ALL_SIGNALS, true);
    t.host().take_change_sets();

    let snapshot = t.runtime().snapshot();
    // State moves on after the snapshot ...
    call_counter(&t, h1, ADD, 3, &enc(&100_i32));
    call_counter(&t, h2, ADD, 4, &enc(&100_i32));
    t.host().take_change_sets();

    // ... and restore takes it back.
    t.runtime().restore(&snapshot).unwrap();
    assert_eq!(get(&t, h1), 6);
    assert_eq!(get(&t, h2), 7);
    assert_eq!(
        decode_body::<String>(&call_counter(&t, h1, LABEL, 5, &[])),
        "a"
    );

    // The observed store's signals were re-emitted, in one change-set; h2 (unobserved) was not.
    let cs = single(&t);
    assert_eq!(
        cs.entries,
        [
            entry(h1, COUNT_SIGNAL, enc(&6_i32)),
            entry(h1, LABEL_SIGNAL, enc("a"))
        ]
    );

    // Non-store objects do not survive: their handles are stale (status 5).
    let reply = reply_of(
        t.runtime(),
        &keel_runtime::testing::call_payload(
            keel_wire::payload::CallTarget::LazyPage {
                handle: lazy,
                offset: 0,
                limit: 1,
            },
            6,
            &[],
        ),
    );
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    assert!(reason_of(&reply).contains("stale"), "{}", reason_of(&reply));

    // The restored store is live: observation survived, writes deliver again.
    call_counter(&t, h1, ADD, 7, &enc(&1_i32));
    assert_eq!(single(&t).entries, [entry(h1, COUNT_SIGNAL, enc(&7_i32))]);
    call_counter(&t, h2, ADD, 8, &enc(&1_i32));
    assert_eq!(t.host().change_set_count(), 0, "h2 was never observed");
}

#[test]
fn restore_reobserves_exactly_what_was_observed() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 3, "x");
    t.runtime().observe(h.0, LABEL_SIGNAL, true);
    t.host().take_change_sets();
    let snapshot = t.runtime().snapshot();
    t.runtime().restore(&snapshot).unwrap();
    assert_eq!(single(&t).entries, [entry(h, LABEL_SIGNAL, enc("x"))]);
}

#[test]
fn restore_into_a_fresh_runtime_makes_the_hosts_handles_valid_again() {
    let old = TestRuntime::new();
    let h1 = new_counter(&old, 5, "a");
    let _skip = new_counter(&old, 0, "released");
    let h3 = new_counter(&old, 9, "c");
    old.runtime().release(_skip.0);
    let snapshot = old.runtime().snapshot();
    drop(old); // the "crash"

    let fresh = TestRuntime::new();
    fresh.runtime().restore(&snapshot).unwrap();
    assert_eq!(fresh.runtime().objects().live(), 2);
    assert_eq!(get(&fresh, h1), 5);
    assert_eq!(get(&fresh, h3), 9);
    // Nothing was observed in the fresh runtime, so nothing was emitted; the host re-observes.
    assert_eq!(fresh.host().change_set_count(), 0);
    fresh.runtime().observe(h3.0, COUNT_SIGNAL, true);
    assert_eq!(
        single(&fresh).entries,
        [entry(h3, COUNT_SIGNAL, enc(&9_i32))]
    );

    // New objects never collide with restored handles, and never reuse a generation a stale
    // handle from before the crash could still hold.
    let newer = new_counter(&fresh, 1, "new");
    assert!(newer != h1 && newer != h3);
    assert!(newer.generation() > h1.generation().max(h3.generation()));
    assert_eq!(get(&fresh, h1), 5);
}

#[test]
fn restore_rejects_bad_snapshots_and_leaves_the_runtime_unchanged() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 5, "a");
    let good = t.runtime().snapshot();

    // Truncated and trailing bytes.
    assert!(matches!(
        t.runtime().restore(&good[..good.len() - 1]),
        Err(RestoreError::Decode(_))
    ));
    let mut long = good.clone();
    long.push(0);
    assert!(matches!(
        t.runtime().restore(&long),
        Err(RestoreError::Decode(WireError::TrailingBytes { .. }))
    ));
    assert!(matches!(
        t.runtime().restore(&[]),
        Err(RestoreError::Decode(_))
    ));

    let encode = |stores: Vec<StoreSnapshot>| {
        let mut w = Writer::new();
        Snapshot { stores }.encode(&mut w);
        w.into_vec()
    };
    let store = |handle: Handle, type_id: u32| StoreSnapshot {
        handle,
        type_id,
        signals: vec![(COUNT_SIGNAL, enc(&1_i32)), (LABEL_SIGNAL, enc("z"))],
    };
    let counter_type = ids::type_id("Counter");

    // Unknown store type.
    assert_eq!(
        t.runtime()
            .restore(&encode(vec![store(Handle::new(4, 1), 0xdead_beef)])),
        Err(RestoreError::UnknownStoreType {
            type_id: 0xdead_beef
        })
    );
    // Null, generation-0 and duplicate handles; absurd indices.
    for bad in [Handle::NULL, Handle::new(1, 0), Handle::new(u32::MAX, 1)] {
        assert_eq!(
            t.runtime().restore(&encode(vec![store(bad, counter_type)])),
            Err(RestoreError::BadHandle { handle: bad.0 }),
            "{bad:?}"
        );
    }
    let dup = Handle::new(2, 1);
    assert_eq!(
        t.runtime().restore(&encode(vec![
            store(dup, counter_type),
            store(dup, counter_type)
        ])),
        Err(RestoreError::BadHandle { handle: dup.0 })
    );
    // A store's own decoder rejecting its values, and one that panics.
    let mut broken = store(Handle::new(3, 1), counter_type);
    broken.signals = vec![(COUNT_SIGNAL, vec![1])]; // i32 needs 4 bytes
    assert!(matches!(
        t.runtime().restore(&encode(vec![broken])),
        Err(RestoreError::Store { type_id, source: WireError::UnexpectedEof { .. } }) if type_id == counter_type
    ));
    assert!(matches!(
        t.runtime()
            .restore(&encode(vec![store(Handle::new(3, 1), REJECTING)])),
        Err(RestoreError::Store {
            type_id: REJECTING,
            ..
        })
    ));
    assert_eq!(
        t.runtime()
            .restore(&encode(vec![store(Handle::new(3, 1), PANICKY)])),
        Err(RestoreError::Panicked {
            type_id: PANICKY,
            message: "restore kaboom".into()
        })
    );
    // The good store among bad ones is not half-applied either.
    assert!(
        t.runtime()
            .restore(&encode(vec![
                store(Handle::new(6, 1), counter_type),
                store(Handle::new(7, 1), 0xdead_beef)
            ]))
            .is_err()
    );

    // After all of that, the original state is intact.
    assert_eq!(t.runtime().objects().live(), 1);
    assert_eq!(get(&t, h), 5);
    assert_eq!(t.runtime().snapshot(), good);
}

#[test]
fn restore_after_shutdown_is_an_error() {
    let t = TestRuntime::new();
    t.runtime().shutdown();
    assert_eq!(
        t.runtime().restore(&[0, 0, 0, 0]),
        Err(RestoreError::ShutDown)
    );
}

#[test]
fn restore_detaches_stores_that_tasks_still_hold() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 1, "");
    t.runtime().observe(h.0, ALL_SIGNALS, true);
    let old = t.runtime().object::<Counter>(h.0).unwrap();
    let snapshot = t.runtime().snapshot();
    t.runtime().restore(&snapshot).unwrap();
    t.host().take_change_sets();

    // The detached original must not talk to the host under the handle the new store owns.
    old.count.set(1000);
    assert_eq!(t.host().change_set_count(), 0);
    assert_eq!(get(&t, h), 1);
}

#[test]
fn restore_reentrancy_is_refused_not_deadlocked() {
    // A host that restores from inside a reply callback (which runs under the core lock).
    use keel_runtime::{Host, PortCallOutcome};
    use std::sync::OnceLock;
    struct Restoring {
        rt: OnceLock<std::sync::Weak<Runtime>>,
        result: parking_lot::Mutex<Option<Result<(), RestoreError>>>,
    }
    impl Host for Restoring {
        fn reply(&self, _: u32, _: &[u8]) {
            if let Some(rt) = self.rt.get().and_then(std::sync::Weak::upgrade) {
                *self.result.lock() = Some(rt.restore(&[0, 0, 0, 0]));
            }
        }
        fn change_set(&self, _: &[u8]) {}
        fn stream_item(&self, _: u32, _: &[u8]) {}
        fn port_call(&self, _: u32, _: u32, _: u32, _: &[u8]) -> PortCallOutcome {
            PortCallOutcome::Unavailable
        }
        fn log(&self, _: u8, _: &str, _: &str) {}
    }
    let host = Arc::new(Restoring {
        rt: OnceLock::new(),
        result: parking_lot::Mutex::new(None),
    });
    let rt = Runtime::new(
        RuntimeConfig {
            core_threads: 0,
            ..RuntimeConfig::default()
        },
        host.clone(),
    )
    .unwrap();
    host.rt.set(Arc::downgrade(&rt)).unwrap();
    let payload = keel_runtime::testing::call_payload(
        function_target(SUM),
        1,
        &args(|w| {
            use keel_wire::Encode;
            1_i32.encode(w);
            1_i32.encode(w);
        }),
    );
    assert_eq!(rt.call(&payload), 0);
    assert_eq!(*host.result.lock(), Some(Err(RestoreError::Reentrant)));
    rt.shutdown();
}

// ----- observe under a self-writing computed (signals re-review R2) -----------------------

/// A store whose computed writes its own input until it reaches 50: `observe` cannot settle
/// it within its pass cap, so the entries go out capped and the runtime's transaction must
/// deliver the rest as follow-up change-sets. The host applies everything in order and must
/// end exactly where the core is.
#[test]
fn r2_observe_past_the_pass_cap_converges_through_follow_up_change_sets() {
    use keel_runtime::StoreObject;
    use keel_signals::{Computed, Signal, StoreCell};

    struct Chaser {
        cell: Arc<StoreCell>,
        _n: Signal<i32>,
        _chase: Computed<i32>,
    }
    impl keel_runtime::KeelObject for Chaser {
        const TYPE_ID: u32 = ids::type_id("Chaser");
        const NAME: &'static str = "Chaser";
    }
    impl StoreObject for Chaser {
        fn cell(&self) -> &Arc<StoreCell> {
            &self.cell
        }
        fn restore(_ctx: keel_runtime::Ctx, _r: &mut Reader<'_>) -> Result<Self, WireError> {
            unreachable!("not snapshotted in this test")
        }
    }

    let t = TestRuntime::new();
    let cell = StoreCell::new(<Chaser as keel_runtime::KeelObject>::TYPE_ID);
    let n = Signal::new(0_i32);
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
    let handle = t.runtime().insert_store(Arc::new(Chaser {
        cell,
        _n: n,
        _chase: chase,
    }));

    t.runtime().observe(handle.0, ALL_SIGNALS, true);
    t.run_pending();

    // Replay every delivery in order; the last value the host holds for signal 0 must be the
    // core's settled value, however many change-sets it took to get there.
    let mut host_n = None;
    for cs in t.host().take_decoded_change_sets() {
        for e in cs.entries {
            if e.handle == handle && e.signal_id == 0 {
                host_n = Some(Reader::new(&e.value).read_i32().unwrap());
            }
        }
    }
    assert_eq!(host_n, Some(50), "the host converged on the core's value");
}
