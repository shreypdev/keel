//! Calls and streams in flight across a restore (ADR-023; review finding M3).
//!
//! A restore replaces every store. A call that was running on the old store used to finish on it
//! and reply `Ok` for a write that the restored store, which now owns the handle, never saw.

mod common;

use std::time::Duration;

use common::*;
use undra_runtime::testing::TestRuntime;
use undra_wire::payload::{CallTarget, ReplyStatus, StreamFlag};

fn stat(t: &TestRuntime, key: &str) -> u64 {
    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    stats[key].as_u64().unwrap()
}

/// The review's T10.
#[test]
fn m3_an_async_call_in_flight_across_a_restore_replies_cancelled_not_ok() {
    let t = TestRuntime::new();
    let rt = t.runtime().clone();
    let h = new_counter(&t, 1, "c");
    let snapshot = rt.snapshot();
    assert_eq!(t.call(counter_target(h, SLOW_ADD), 50, &enc(&100_i32)), 0);
    t.run_pending();
    assert_eq!(stat(&t, "active_calls"), 1);
    t.take_replies();

    rt.restore(&snapshot).unwrap();
    // The call is answered at once, not when the old store finishes.
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert_eq!(
        (replies[0].call_id, replies[0].status),
        (50, ReplyStatus::Cancelled)
    );
    assert!(replies[0].body.is_empty());
    assert_eq!(stat(&t, "active_calls"), 0);
    assert_eq!(stat(&t, "tasks"), 0, "the task was dropped");

    // Nothing more arrives when the old future's timer would have fired, and the store at the
    // handle holds its restored value.
    t.advance(Duration::from_millis(50));
    assert!(t.take_replies().is_empty(), "exactly one terminal reply");
    assert_eq!(decode_body::<i32>(&call_counter(&t, h, GET, 51, &[])), 1);
}

#[test]
fn m3_a_stream_in_flight_across_a_restore_ends_with_one_failed_item() {
    let t = TestRuntime::new();
    let rt = t.runtime().clone();
    let h = new_counter(&t, 1, "c");
    let snapshot = rt.snapshot();
    assert_eq!(t.call(counter_target(h, TICKS), 60, &enc(&1000_u32)), 0);
    t.run_pending();
    let opened = t.take_replies();
    assert_eq!(opened[0].status, ReplyStatus::StreamOpened);
    rt.stream_credit(60, 2);
    t.run_pending();
    assert_eq!(
        t.host().take_stream_items().len(),
        2,
        "two items were delivered"
    );

    rt.restore(&snapshot).unwrap();
    let items = t.host().take_stream_items();
    assert_eq!(items.len(), 1, "{items:?}");
    // ADR-036: flag 3, cancelled by the core, whatever the stream's own error type is.
    assert_eq!((items[0].call_id, items[0].flag), (60, StreamFlag::Failed));
    let failure =
        undra_wire::payload::StreamFailure::decode(&mut undra_wire::Reader::new(&items[0].body))
            .unwrap();
    assert_eq!(failure.status, ReplyStatus::Cancelled, "{items:?}");
    assert!(failure.message.contains("restore"), "{failure:?}");
    assert_eq!(stat(&t, "open_streams"), 0);
    // Credit for the ended stream is ignored; nothing follows.
    rt.stream_credit(60, 100);
    t.run_pending();
    assert!(t.host().take_stream_items().is_empty());
}

#[test]
fn m3_calls_without_a_receiver_carry_on_across_a_restore() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 1, "c");
    let snapshot = t.runtime().snapshot();
    // An async free function has no receiver object to lose.
    assert_eq!(t.call(function_target(SLOW_FN), 70, &[]), 0);
    t.runtime().restore(&snapshot).unwrap();
    assert!(t.take_replies().is_empty(), "not cancelled");
    t.run_pending();
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(decode_body::<i32>(&replies[0]), 7);
    assert_eq!(decode_body::<i32>(&call_counter(&t, h, GET, 71, &[])), 1);
}

/// A call on an object that was already released before the restore is not something the
/// restore changed: its handle names nothing new, and it finishes as it always did.
#[test]
fn m3_a_call_on_an_object_released_before_the_restore_is_left_alone() {
    let t = TestRuntime::new();
    let rt = t.runtime().clone();
    let keep = new_counter(&t, 5, "keep");
    let snapshot = rt.snapshot();
    let gone = new_counter(&t, 1, "gone");
    assert_eq!(t.call(counter_target(gone, SLOW_ADD), 80, &enc(&1_i32)), 0);
    t.run_pending();
    rt.release(gone.0); // the task still holds the object
    rt.restore(&snapshot).unwrap();
    assert!(t.take_replies().is_empty());
    t.advance(Duration::from_millis(50));
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].status, ReplyStatus::Ok);
    assert_eq!(decode_body::<i32>(&call_counter(&t, keep, GET, 81, &[])), 5);
}

/// A cancel that the host issues itself, racing the restore, still yields exactly one reply.
#[test]
fn m3_a_call_the_host_already_cancelled_is_not_answered_twice() {
    let t = TestRuntime::new();
    let rt = t.runtime().clone();
    let h = new_counter(&t, 1, "c");
    let snapshot = rt.snapshot();
    assert_eq!(
        t.call(
            CallTarget::Method {
                handle: h,
                method_id: FOREVER
            },
            90,
            &[]
        ),
        0
    );
    t.run_pending();
    rt.cancel(90);
    rt.restore(&snapshot).unwrap();
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert_eq!(replies[0].status, ReplyStatus::Cancelled);
}
