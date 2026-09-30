//! Change-sets: the initial value on observe, one per transaction, in commit order.
#![cfg(feature = "server")]

mod common;

use common::*;
use keel::wire::Kind;
use keel::wire::payload::{ChangeOp, ReplyStatus};

/// The `i32` a `count` entry carries.
fn count_of(cs: &keel::wire::payload::ChangeSet) -> i32 {
    let entry = cs
        .entries
        .iter()
        .find(|e| e.signal_id == COUNT_SIGNAL)
        .expect("a count entry");
    assert_eq!(entry.op, ChangeOp::Full);
    dec::<i32>(&entry.value)
}

#[test]
fn observing_delivers_the_current_value_first() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(7);
    client.observe(handle, COUNT_SIGNAL, true);
    let frame = client.recv_kind(Kind::ChangeSet);
    let cs = change_set(&frame);
    assert_eq!(cs.entries.len(), 1);
    assert_eq!(cs.entries[0].handle.0, handle);
    assert_eq!(count_of(&cs), 7);
}

#[test]
fn observing_every_signal_delivers_them_all() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(1);
    client.observe(handle, u32::MAX, true);
    let cs = change_set(&client.recv_kind(Kind::ChangeSet));
    let mut ids: Vec<u32> = cs.entries.iter().map(|e| e.signal_id).collect();
    ids.sort_unstable();
    assert_eq!(ids, [0, 1]);
}

#[test]
fn each_write_is_a_change_set_and_they_arrive_in_commit_order() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    client.observe(handle, COUNT_SIGNAL, true);
    for n in 1..=20 {
        let (status, _) = client.method(handle, ADD, &enc(&1_i32));
        assert_eq!(status, ReplyStatus::Ok, "add {n}");
    }
    let counts: Vec<i32> = change_sets(&client).iter().map(count_of).collect();
    assert_eq!(
        counts,
        (0..=20).collect::<Vec<_>>(),
        "the initial value, then 1..=20"
    );

    let txns: Vec<u64> = change_sets(&client).iter().map(|cs| cs.txn_id).collect();
    assert!(
        txns.windows(2).all(|w| w[0] < w[1]),
        "txn ids increase: {txns:?}"
    );

    // Per-direction sequence numbers increase without gaps across every kind of message.
    let seqs: Vec<u32> = client.seen.iter().map(|frame| frame.seq).collect();
    assert_eq!(seqs, (0..seqs.len() as u32).collect::<Vec<_>>());
}

#[test]
fn a_change_set_precedes_the_reply_of_the_call_that_caused_it() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    client.observe(handle, COUNT_SIGNAL, true);
    let id = client.next_call_id();
    client.send_call(
        keel::wire::payload::CallTarget::Method {
            handle: keel::wire::Handle(handle),
            method_id: ADD,
        },
        id,
        &enc(&5_i32),
    );
    client.await_reply(id);
    let kinds: Vec<Kind> = client
        .seen
        .iter()
        .map(|frame| frame.kind)
        .filter(|kind| matches!(kind, Kind::ChangeSet | Kind::Reply))
        .collect();
    // hello aside: constructor reply, initial change-set, then (change-set, reply) for add.
    assert_eq!(
        kinds,
        [Kind::Reply, Kind::ChangeSet, Kind::ChangeSet, Kind::Reply]
    );
}

#[test]
fn a_transaction_is_one_change_set_never_split() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    client.observe(handle, u32::MAX, true);
    client.recv_kind(Kind::ChangeSet); // the initial values
    let (status, _) = client.method(handle, ADD_AND_LABEL, &enc(&3_i32));
    assert_eq!(status, ReplyStatus::Ok);
    let sets = change_sets(&client);
    assert_eq!(sets.len(), 2, "initial + one for the whole transaction");
    let mut ids: Vec<u32> = sets[1].entries.iter().map(|e| e.signal_id).collect();
    ids.sort_unstable();
    assert_eq!(ids, [0, 1]);
}

#[test]
fn unobserving_stops_the_flow() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    client.observe(handle, COUNT_SIGNAL, true);
    client.method(handle, ADD, &enc(&1_i32));
    client.observe(handle, COUNT_SIGNAL, false);
    client.method(handle, ADD, &enc(&1_i32));
    client.method(handle, ADD, &enc(&1_i32));
    let counts: Vec<i32> = change_sets(&client).iter().map(count_of).collect();
    assert_eq!(
        counts,
        [0, 1],
        "nothing after the observation was switched off"
    );
}

#[test]
fn a_write_made_by_another_thread_of_the_core_is_forwarded() {
    // The runtime's own thread (a slow async method) commits; the bridge is called from there.
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    client.observe(handle, COUNT_SIGNAL, true);
    client.recv_kind(Kind::ChangeSet);
    let (status, _) = client.method(handle, SLOW_ADD, &enc(&2_i32));
    assert_eq!(status, ReplyStatus::Ok);
    let counts: Vec<i32> = change_sets(&client).iter().map(count_of).collect();
    assert_eq!(counts, [0, 2]);
}

#[test]
fn a_write_made_from_outside_any_client_is_forwarded_to_the_attached_one() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    client.observe(handle, COUNT_SIGNAL, true);
    client.recv_kind(Kind::ChangeSet);
    // The embedding app writes into its own core; the client observing it sees the change.
    {
        let _scope = f.rt.ctx().enter();
        let counter =
            f.rt.object::<Counter>(handle)
                .expect("the client's counter is in the object table");
        counter.add(4);
    }
    let cs = change_set(&client.recv_kind(Kind::ChangeSet));
    assert_eq!(count_of(&cs), 4);
}
