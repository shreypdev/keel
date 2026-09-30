//! Streams: status 4, credit-based flow control, end and error markers, cancellation.

mod common;

use common::*;
use keel_runtime::testing::{StreamRecord, TestRuntime};
use keel_wire::payload::{ReplyStatus, StreamFlag};
use keel_wire::{Decode, Reader};

fn items(records: &[StreamRecord]) -> Vec<(StreamFlag, Vec<u8>)> {
    records.iter().map(|r| (r.flag, r.body.clone())).collect()
}

fn item(n: i32) -> (StreamFlag, Vec<u8>) {
    (StreamFlag::Item, enc(&n))
}

fn end() -> (StreamFlag, Vec<u8>) {
    (StreamFlag::End, Vec::new())
}

fn open(t: &TestRuntime, method: u32, n: u32, call_id: u32) -> keel_wire::Handle {
    let handle = new_counter(t, 0, "");
    assert_eq!(t.call(counter_target(handle, method), call_id, &enc(&n)), 0);
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(
        (replies[0].call_id, replies[0].status),
        (call_id, ReplyStatus::StreamOpened)
    );
    assert!(replies[0].body.is_empty());
    handle
}

#[test]
fn items_flow_only_when_credited_and_the_stream_ends_with_flag_1() {
    let t = TestRuntime::new();
    open(&t, TICKS, 3, 5);

    // Initial credit is 0: nothing flows, however often the executor runs.
    t.run_pending();
    t.run_pending();
    assert!(t.host().take_stream_items().is_empty());

    t.runtime().stream_credit(5, 2);
    t.run_pending();
    assert_eq!(items(&t.host().take_stream_items()), [item(0), item(1)]);

    // No more credit, no more items.
    t.run_pending();
    assert!(t.host().take_stream_items().is_empty());

    t.runtime().stream_credit(5, 1);
    t.run_pending();
    let last = t.host().take_stream_items();
    assert_eq!(items(&last), [item(2), end()]);
    assert!(last.iter().all(|r| r.call_id == 5));

    // The call is over: its slot is free and further credit is ignored.
    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    assert_eq!(
        (
            stats["active_calls"].clone(),
            stats["open_streams"].clone(),
            stats["tasks"].clone()
        ),
        (0.into(), 0.into(), 0.into())
    );
    t.runtime().stream_credit(5, 10);
    t.run_pending();
    assert!(t.host().take_stream_items().is_empty());
}

#[test]
fn credit_granted_before_the_driver_runs_accumulates() {
    let t = TestRuntime::new();
    open(&t, TICKS, 10, 1);
    t.runtime().stream_credit(1, 1);
    t.runtime().stream_credit(1, 1);
    t.runtime().stream_credit(1, 1);
    t.run_pending();
    assert_eq!(
        items(&t.host().take_stream_items()),
        [item(0), item(1), item(2)]
    );
}

#[test]
fn an_empty_stream_ends_without_any_credit() {
    let t = TestRuntime::new();
    open(&t, TICKS, 0, 2);
    t.run_pending();
    assert_eq!(items(&t.host().take_stream_items()), [end()]);
}

#[test]
fn the_end_marker_needs_no_credit_after_the_last_item() {
    // Exactly as much credit as items: the host still hears the end.
    let t = TestRuntime::new();
    open(&t, TICKS, 2, 3);
    t.runtime().stream_credit(3, 2);
    t.run_pending();
    assert_eq!(
        items(&t.host().take_stream_items()),
        [item(0), item(1), end()]
    );
}

#[test]
fn a_stream_error_is_flag_2_and_closes_the_stream() {
    let t = TestRuntime::new();
    open(&t, TICKS_FAIL, 4, 4); // fails at item 2
    t.runtime().stream_credit(4, 10);
    t.run_pending();
    let got = t.host().take_stream_items();
    assert_eq!(got.len(), 3, "{got:?}");
    assert_eq!(items(&got[..2]), [item(0), item(1)]);
    assert_eq!(got[2].flag, StreamFlag::Error);
    assert_eq!(
        String::decode(&mut Reader::new(&got[2].body)).unwrap(),
        "stream failed"
    );
    // Closed: no end marker, no more items, call slot released.
    t.runtime().stream_credit(4, 10);
    t.run_pending();
    assert!(t.host().take_stream_items().is_empty());
    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    assert_eq!(stats["open_streams"], 0);
}

#[test]
fn cancel_closes_a_stream_without_another_message() {
    let t = TestRuntime::new();
    open(&t, TICKS, 100, 6);
    t.runtime().stream_credit(6, 1);
    t.run_pending();
    assert_eq!(items(&t.host().take_stream_items()), [item(0)]);

    t.runtime().cancel(6);
    assert!(
        t.take_replies().is_empty(),
        "the host closed it; no status 3 for a stream"
    );
    assert!(t.host().take_stream_items().is_empty());
    t.runtime().stream_credit(6, 50);
    t.run_pending();
    assert!(t.host().take_stream_items().is_empty());
    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    assert_eq!(
        (stats["open_streams"].clone(), stats["tasks"].clone()),
        (0.into(), 0.into())
    );
}

#[test]
fn a_panicking_stream_becomes_an_error_item_and_poisons_the_store() {
    let t = TestRuntime::new();
    let handle = open(&t, TICKS_PANIC, 4, 7); // panics at item 2
    t.runtime().stream_credit(7, 10);
    t.run_pending();
    let got = t.host().take_stream_items();
    assert_eq!(items(&got[..2]), [item(0), item(1)]);
    assert_eq!(got[2].flag, StreamFlag::Error);
    let message = String::decode(&mut Reader::new(&got[2].body)).unwrap();
    assert!(message.contains("stream kaboom"), "{message}");
    // The runtime and the store keep working.
    assert_eq!(
        call_counter(&t, handle, GET, 8, &[]).status,
        ReplyStatus::Ok
    );
    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    assert_eq!(stats["panics"], 1);
}

#[test]
fn two_streams_are_independent() {
    let t = TestRuntime::new();
    let a = new_counter(&t, 0, "");
    assert_eq!(t.call(counter_target(a, TICKS), 1, &enc(&3_u32)), 0);
    assert_eq!(t.call(counter_target(a, TICKS), 2, &enc(&3_u32)), 0);
    t.take_replies();
    t.runtime().stream_credit(2, 1);
    t.run_pending();
    let got = t.host().take_stream_items();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].call_id, 2);
    t.runtime().stream_credit(1, 3);
    t.run_pending();
    let got = t.host().take_stream_items();
    assert_eq!(got.len(), 4);
    assert!(got.iter().all(|r| r.call_id == 1));
    assert_eq!(got[3].flag, StreamFlag::End);
}

#[test]
fn credit_saturates_instead_of_overflowing() {
    let t = TestRuntime::new();
    open(&t, TICKS, 3, 9);
    t.runtime().stream_credit(9, u32::MAX);
    t.runtime().stream_credit(9, u32::MAX);
    t.run_pending();
    assert_eq!(
        items(&t.host().take_stream_items()),
        [item(0), item(1), item(2), end()]
    );
}

#[test]
fn credit_for_unknown_calls_and_plain_async_calls_is_ignored() {
    let t = TestRuntime::new();
    t.runtime().stream_credit(404, 5);
    let handle = new_counter(&t, 0, "");
    assert_eq!(t.call(counter_target(handle, FOREVER), 3, &[]), 0);
    t.runtime().stream_credit(3, 5);
    t.run_pending();
    assert!(t.host().take_stream_items().is_empty());
    t.runtime().cancel(3);
}

#[test]
fn stream_reply_precedes_every_item() {
    // Status 4 is sent by `call` itself, before the driver can run.
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    t.runtime()
        .call(&counter_call_payload(handle, TICKS, 2, &enc(&1_u32)));
    assert_eq!(t.host().reply_count(), 1);
    assert!(t.host().take_stream_items().is_empty());
}
