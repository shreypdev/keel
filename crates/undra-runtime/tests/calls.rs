//! The call path: sync and async dispatch, replies, cancellation, panics, bad requests.

mod common;

use std::time::Duration;

use common::*;
use undra_meta::ids;
use undra_runtime::testing::{TestRuntime, call_payload};
use undra_runtime::{Handle, Runtime, RuntimeConfig};
use undra_wire::payload::{CallTarget, ReplyStatus};
use undra_wire::{Bytes, Encode, Reader, Writer};

// ----- sync round trip --------------------------------------------------------------------

#[test]
fn sync_call_round_trip_through_a_constructed_store() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 5, "five");
    assert!(!handle.is_null());
    assert_eq!(handle.generation(), 1);
    assert_eq!(t.runtime().objects().live(), 1);

    let got: i32 = decode_body(&call_counter(&t, handle, GET, 2, &[]));
    assert_eq!(got, 5);
    let added: i32 = decode_body(&call_counter(&t, handle, ADD, 3, &enc(&3_i32)));
    assert_eq!(added, 8);
    let got: i32 = decode_body(&call_counter(&t, handle, GET, 4, &[]));
    assert_eq!(got, 8);
    let label: String = decode_body(&call_counter(&t, handle, LABEL, 5, &[]));
    assert_eq!(label, "five");
    // Replies of call_sync are returned, not sent through the host.
    assert!(t.take_replies().is_empty());
}

#[test]
fn the_reply_echoes_the_call_id() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    for call_id in [1_u32, 77, u32::MAX] {
        let reply = call_counter(&t, handle, GET, call_id, &[]);
        assert_eq!(reply.call_id, call_id);
    }
}

#[test]
fn free_functions_dispatch_by_method_id() {
    let t = TestRuntime::new();
    let reply = t.call_sync(
        function_target(SUM),
        1,
        &args(|w| {
            2_i32.encode(w);
            40_i32.encode(w);
        }),
    );
    assert_eq!(decode_body::<i32>(&reply), 42);
    for data in [
        Vec::new(),
        vec![0, 1, 2],
        "h\u{e9}llo \u{1F30A}".as_bytes().to_vec(),
    ] {
        let reply = t.call_sync(function_target(ECHO), 2, &enc(&Bytes(data.clone())));
        assert_eq!(decode_body::<Bytes>(&reply).0, data);
    }
}

#[test]
fn typed_errors_are_status_1_with_the_error_value() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    let reply = call_counter(&t, handle, FAIL, 2, &enc(&404_u16));
    assert_eq!(reply.status, ReplyStatus::Error);
    assert_eq!(reply.body, enc(&404_u16));
}

#[test]
fn call_with_a_sync_result_replies_through_the_host_before_returning() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 1, "");
    let accepted = t.call(counter_target(handle, ADD), 9, &enc(&4_i32));
    assert_eq!(accepted, 0);
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].call_id, 9);
    assert_eq!(decode_body::<i32>(&replies[0]), 5);
}

#[test]
fn call_ids_are_not_reserved_by_sync_results() {
    // A finished call frees its id at once.
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    assert_eq!(t.call(counter_target(handle, GET), 5, &[]), 0);
    assert_eq!(t.call(counter_target(handle, GET), 5, &[]), 0);
    assert_eq!(t.take_replies().len(), 2);
}

// ----- bad requests -----------------------------------------------------------------------

#[test]
fn unknown_method_function_and_type_are_status_5() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    let reply = call_counter(&t, handle, EVEN_MORE, 2, &[]);
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    assert!(
        reason_of(&reply).contains("unknown method"),
        "{}",
        reason_of(&reply)
    );

    let reply = t.call_sync(function_target(ids::function_id("nope")), 3, &[]);
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    assert!(reason_of(&reply).contains("unknown function"));

    let reply = t.call_sync(
        CallTarget::Constructor {
            type_id: ids::type_id("NoSuchType"),
            method_id: 1,
        },
        4,
        &[],
    );
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    assert!(reason_of(&reply).contains("unknown object type"));
}

#[test]
fn undecodable_arguments_are_status_5_with_a_reason() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    // Too short.
    let reply = call_counter(&t, handle, ADD, 2, &[1, 2]);
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    assert!(
        reason_of(&reply).contains("bad arguments"),
        "{}",
        reason_of(&reply)
    );
    // Trailing bytes.
    let mut long = enc(&1_i32);
    long.push(0);
    let reply = call_counter(&t, handle, ADD, 3, &long);
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    // Nothing was changed by the rejected calls.
    assert_eq!(
        decode_body::<i32>(&call_counter(&t, handle, GET, 4, &[])),
        0
    );
}

#[test]
fn malformed_call_payloads_are_status_5_from_call_sync_and_rejected_by_call() {
    let t = TestRuntime::new();
    let rt = t.runtime();
    for payload in [&[][..], &[9][..], &[1, 0, 0][..], &[0xff; 30][..]] {
        let reply = undra_runtime::testing::decode_reply(&rt.call_sync(payload));
        assert_eq!(reply.status, ReplyStatus::BadRequest, "{payload:?}");
        assert_eq!(reply.call_id, 0, "no call id could be read");
        assert_eq!(rt.call(payload), 5);
    }
    assert!(
        t.take_replies().is_empty(),
        "call() rejects without replying"
    );
}

#[test]
fn stale_null_and_out_of_range_handles_are_status_5() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    t.runtime().release(handle.0);
    let stale = call_counter(&t, handle, GET, 2, &[]);
    assert_eq!(stale.status, ReplyStatus::BadRequest);
    assert!(
        reason_of(&stale).contains("stale handle"),
        "{}",
        reason_of(&stale)
    );

    let null = call_counter(&t, Handle::NULL, GET, 3, &[]);
    assert_eq!(null.status, ReplyStatus::BadRequest);
    assert!(reason_of(&null).contains("null handle"));

    let far = call_counter(&t, Handle::new(999, 1), GET, 4, &[]);
    assert_eq!(far.status, ReplyStatus::BadRequest);
    assert!(reason_of(&far).contains("unknown handle"));
}

#[test]
fn a_handle_of_the_wrong_type_is_status_5() {
    let t = TestRuntime::new();
    let lazy = Handle(decode_body::<u64>(&t.call_sync(
        function_target(MAKE_LAZY),
        1,
        &enc(&3_u32),
    )));
    let reply = call_counter(&t, lazy, GET, 2, &[]);
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    assert!(
        reason_of(&reply).contains("no dispatcher"),
        "{}",
        reason_of(&reply)
    );
}

#[test]
fn call_rejects_call_id_zero_and_duplicates_in_flight() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    assert_eq!(
        t.call(counter_target(handle, GET), 0, &[]),
        5,
        "call_id 0 is reserved"
    );
    assert!(t.take_replies().is_empty());

    // An async call keeps its id until it finishes.
    assert_eq!(t.call(counter_target(handle, SLOW_ADD), 7, &enc(&1_i32)), 0);
    assert_eq!(
        t.call(counter_target(handle, GET), 7, &[]),
        5,
        "duplicate id in flight"
    );
    assert!(t.take_replies().is_empty());
    t.advance(Duration::from_millis(10));
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].call_id, 7);
    // Now the id is free again.
    assert_eq!(t.call(counter_target(handle, GET), 7, &[]), 0);
}

// ----- async ------------------------------------------------------------------------------

#[test]
fn async_call_replies_through_the_host_when_it_finishes() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 10, "");
    let accepted = t.call(counter_target(handle, SLOW_ADD), 21, &enc(&5_i32));
    assert_eq!(accepted, 0);
    assert!(
        t.take_replies().is_empty(),
        "nothing yet: the method sleeps"
    );

    assert_eq!(t.run_pending(), 1, "the first poll starts the sleep");
    assert!(t.take_replies().is_empty());
    t.advance(Duration::from_millis(9));
    assert!(t.take_replies().is_empty());
    t.advance(Duration::from_millis(1));

    let replies = t.take_replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].call_id, 21);
    assert_eq!(decode_body::<i32>(&replies[0]), 15);
    assert_eq!(
        decode_body::<i32>(&call_counter(&t, handle, GET, 22, &[])),
        15
    );
}

#[test]
fn concurrent_async_calls_reply_in_completion_order() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    // The same 10 ms sleep, started in order 1, 2, 3.
    for call_id in 1..=3 {
        assert_eq!(
            t.call(counter_target(handle, SLOW_ADD), call_id, &enc(&1_i32)),
            0
        );
    }
    t.run_pending();
    t.advance(Duration::from_millis(10));
    let ids: Vec<u32> = t.take_replies().iter().map(|r| r.call_id).collect();
    assert_eq!(ids, [1, 2, 3]);
    assert_eq!(
        decode_body::<i32>(&call_counter(&t, handle, GET, 9, &[])),
        3
    );
}

#[test]
fn async_free_function_works_through_call() {
    let t = TestRuntime::new();
    assert_eq!(t.call(function_target(SLOW_FN), 1, &[]), 0);
    t.run_pending();
    let replies = t.take_replies();
    assert_eq!(decode_body::<i32>(&replies[0]), 7);
}

#[test]
fn call_sync_refuses_async_methods_without_running_them() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    let reply = call_counter(&t, handle, SLOW_ADD, 2, &enc(&5_i32));
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    assert!(
        reason_of(&reply).contains("asynchronous"),
        "{}",
        reason_of(&reply)
    );
    let reply = t.call_sync(function_target(SLOW_FN), 3, &[]);
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    // Streams too.
    let reply = call_counter(&t, handle, TICKS, 4, &enc(&3_u32));
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    t.run_pending();
    t.advance(Duration::from_secs(1));
    assert_eq!(
        decode_body::<i32>(&call_counter(&t, handle, GET, 5, &[])),
        0
    );
    assert!(t.runtime().stats_json().contains("\"tasks\":0"));
}

#[test]
fn spawned_detached_tasks_run_on_the_executor() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 1, "");
    assert_eq!(
        call_counter(&t, handle, SPAWN_LATER, 2, &[]).status,
        ReplyStatus::Ok
    );
    assert_eq!(
        decode_body::<i32>(&call_counter(&t, handle, GET, 3, &[])),
        1
    );
    t.run_pending();
    assert_eq!(
        decode_body::<i32>(&call_counter(&t, handle, GET, 4, &[])),
        101
    );
}

// ----- cancellation -----------------------------------------------------------------------

#[test]
fn cancel_replies_status_3_and_drops_the_future() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    assert_eq!(t.call(counter_target(handle, SLOW_ADD), 5, &enc(&1_i32)), 0);
    t.run_pending(); // the sleep is armed
    assert!(t.take_replies().is_empty());
    let counter = t.runtime().object::<Counter>(handle.0).unwrap();
    assert!(
        std::sync::Arc::strong_count(&counter) >= 3,
        "table + future + mine"
    );

    t.runtime().cancel(5);
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(
        (replies[0].call_id, replies[0].status),
        (5, ReplyStatus::Cancelled)
    );
    assert!(replies[0].body.is_empty());
    assert_eq!(
        std::sync::Arc::strong_count(&counter),
        2,
        "the cancelled future (and its captured Arc) is gone"
    );
    // The sleep is cancelled too: time passing does nothing, the counter is untouched.
    t.advance(Duration::from_secs(1));
    assert!(t.take_replies().is_empty());
    assert_eq!(counter.count.get(), 0);
    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    assert_eq!(stats["tasks"], 0);
    assert_eq!(stats["pending_timers"], 0);
    assert_eq!(stats["active_calls"], 0);
    assert_eq!(stats["crossings"]["cancelled"], 1);
}

#[test]
fn cancel_of_finished_or_unknown_calls_is_a_no_op() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    t.runtime().cancel(404);
    assert_eq!(t.call(counter_target(handle, SLOW_ADD), 6, &enc(&1_i32)), 0);
    t.run_pending();
    t.advance(Duration::from_millis(10));
    assert_eq!(t.take_replies().len(), 1);
    t.runtime().cancel(6);
    assert!(
        t.take_replies().is_empty(),
        "no second reply after completion"
    );
    // Cancelling twice replies once.
    assert_eq!(t.call(counter_target(handle, FOREVER), 8, &[]), 0);
    t.runtime().cancel(8);
    t.runtime().cancel(8);
    assert_eq!(t.take_replies().len(), 1);
}

#[test]
fn cancelling_a_call_that_waits_on_a_port_abandons_the_port_call() {
    let t = TestRuntime::new();
    t.host().script_port_async(TEST_PORT, ASK);
    let handle = new_counter(&t, 0, "");
    assert_eq!(
        t.call(counter_target(handle, ASK_PORT), 3, &enc(&21_i32)),
        0
    );
    t.run_pending();
    let port_call = t.host().port_calls().remove(0);
    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    assert_eq!(stats["pending_port_calls"], 1);

    t.runtime().cancel(3);
    assert_eq!(t.take_replies()[0].status, ReplyStatus::Cancelled);
    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    assert_eq!(stats["pending_port_calls"], 0);
    assert_eq!(stats["abandoned_port_calls"], 1);

    // The host's late reply is recognised and discarded.
    t.host().take_logs();
    t.runtime()
        .port_reply(&undra_runtime::testing::port_reply_ok(
            port_call.port_call_id,
            &enc(&1_i32),
        ));
    let logs = t.host().take_logs();
    assert!(
        logs.iter().any(|l| l.message.contains("abandoned")),
        "{logs:?}"
    );
    assert!(t.take_replies().is_empty());
    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    assert_eq!(stats["abandoned_port_calls"], 0);
}

// ----- panics -----------------------------------------------------------------------------

fn panic_body(reply: &undra_runtime::testing::ReplyRecord) -> (String, String) {
    assert_eq!(reply.status, ReplyStatus::Panic, "{reply:?}");
    let mut r = Reader::new(&reply.body);
    let message = r.read_str().unwrap().to_owned();
    let backtrace = r.read_str().unwrap().to_owned();
    r.finish().unwrap();
    (message, backtrace)
}

#[test]
fn panic_in_a_method_is_status_2_and_the_runtime_keeps_working() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 3, "");
    let other = new_counter(&t, 30, "");
    let reply = call_counter(&t, handle, BOOM, 2, &[]);
    let (message, backtrace) = panic_body(&reply);
    assert_eq!(message, "kaboom");
    assert!(backtrace.contains("panicked at"), "{backtrace}");
    assert!(
        backtrace.contains("calls.rs") || backtrace.contains("common"),
        "{backtrace}"
    );

    // Level 5 log with the message.
    let logs = t.host().take_logs();
    assert!(
        logs.iter()
            .any(|l| l.level == 5 && l.message.contains("kaboom")),
        "{logs:?}"
    );
    // Same runtime, same store, still fully usable.
    assert_eq!(
        decode_body::<i32>(&call_counter(&t, handle, ADD, 3, &enc(&1_i32))),
        4
    );
    assert_eq!(
        decode_body::<i32>(&call_counter(&t, other, GET, 4, &[])),
        30
    );
    // Poisoning is recorded (informational) and counted.
    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    assert_eq!(stats["panics"], 1);
    assert_eq!(stats["poisoned_stores"], 1);
    // And through call() the reply comes via the host.
    assert_eq!(t.call(counter_target(handle, BOOM), 9, &[]), 0);
    assert_eq!(panic_body(&t.take_replies()[0]).0, "kaboom");
}

#[test]
fn panic_in_an_async_method_is_status_2_and_frees_the_call() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    assert_eq!(t.call(counter_target(handle, PANIC_ASYNC), 4, &[]), 0);
    assert!(t.take_replies().is_empty());
    t.run_pending();
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].call_id, 4);
    assert_eq!(panic_body(&replies[0]).0, "async kaboom");
    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    assert_eq!(
        (stats["active_calls"].clone(), stats["tasks"].clone()),
        (0.into(), 0.into())
    );
    // The id can be reused and the executor still works.
    assert_eq!(t.call(counter_target(handle, SLOW_ADD), 4, &enc(&2_i32)), 0);
    t.run_pending();
    t.advance(Duration::from_millis(10));
    assert_eq!(decode_body::<i32>(&t.take_replies()[0]), 2);
}

#[test]
fn spawn_blocking_results_and_panics_reach_the_awaiting_call() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    assert_eq!(
        t.call(counter_target(handle, BLOCKING_SQUARE), 1, &enc(&12_i32)),
        0
    );
    t.run_pending();
    assert_eq!(decode_body::<i32>(&t.take_replies()[0]), 144);

    assert_eq!(t.call(counter_target(handle, BLOCKING_BOOM), 2, &[]), 0);
    t.run_pending();
    let replies = t.take_replies();
    assert_eq!(panic_body(&replies[0]).0, "blocking kaboom");
}

#[test]
fn a_panic_in_a_host_callback_is_contained_like_any_other() {
    use std::sync::Arc;
    use undra_runtime::{Host, PortCallOutcome};
    struct Bomb;
    impl Host for Bomb {
        fn reply(&self, _: u32, _: &[u8]) {
            panic!("host reply bomb");
        }
        fn change_set(&self, _: &[u8]) {}
        fn stream_item(&self, _: u32, _: &[u8]) {}
        fn port_call(&self, _: u32, _: u32, _: u32, _: &[u8]) -> PortCallOutcome {
            PortCallOutcome::Unavailable
        }
        fn log(&self, _: u8, _: &str, _: &str) {}
    }
    let rt = Runtime::new(
        RuntimeConfig {
            core_threads: 0,
            ..RuntimeConfig::default()
        },
        Arc::new(Bomb),
    )
    .unwrap();
    // `call()` sends its reply from inside the dispatch guard: the panic is caught there.
    let payload = call_payload(
        function_target(SUM),
        1,
        &args(|w| {
            1_i32.encode(w);
            2_i32.encode(w);
        }),
    );
    assert_eq!(rt.call(&payload), 0);
    // call_sync does not use the host reply path and still works afterwards.
    let reply = undra_runtime::testing::decode_reply(&rt.call_sync(&payload));
    assert_eq!(decode_body::<i32>(&reply), 3);
    rt.shutdown();
}

// ----- Ctx::current -----------------------------------------------------------------------

#[test]
fn ctx_current_is_the_dispatching_runtime() {
    let a = TestRuntime::new();
    let b = TestRuntime::new();
    let ha = new_counter(&a, 0, "");
    let hb = new_counter(&b, 0, "");
    let ida: u64 = decode_body(&call_counter(&a, ha, CURRENT_RUNTIME_ID, 2, &[]));
    let idb: u64 = decode_body(&call_counter(&b, hb, CURRENT_RUNTIME_ID, 2, &[]));
    assert_eq!(ida, a.runtime().id());
    assert_eq!(idb, b.runtime().id());
    assert_ne!(ida, idb);
    // Outside a call there is none.
    assert!(undra_runtime::Ctx::try_current().is_none());
}

// ----- lazy lists -------------------------------------------------------------------------

fn page_call(handle: Handle, offset: u32, limit: u32, call_id: u32) -> Vec<u8> {
    call_payload(
        CallTarget::LazyPage {
            handle,
            offset,
            limit,
        },
        call_id,
        &[],
    )
}

#[test]
fn lazy_pages_are_served_by_the_runtime() {
    let t = TestRuntime::new();
    let lazy = Handle(decode_body::<u64>(&t.call_sync(
        function_target(MAKE_LAZY),
        1,
        &enc(&5_u32),
    )));
    let reply = reply_of(t.runtime(), &page_call(lazy, 1, 2, 2));
    let body = expect_ok(&reply);
    let mut r = Reader::new(body);
    assert_eq!((r.read_u32().unwrap(), r.read_u32().unwrap()), (5, 2));
    assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (10, 20));
    r.finish().unwrap();

    // Past the end: total but no items.
    let reply = reply_of(t.runtime(), &page_call(lazy, 9, 3, 3));
    assert_eq!(expect_ok(&reply), [5, 0, 0, 0, 0, 0, 0, 0]);
    // Through call() as well.
    assert_eq!(t.runtime().call(&page_call(lazy, 0, 1, 4)), 0);
    assert_eq!(t.take_replies()[0].body[..8], [5, 0, 0, 0, 1, 0, 0, 0]);
}

#[test]
fn a_lazy_page_of_a_non_lazy_handle_is_status_5() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    let reply = reply_of(t.runtime(), &page_call(handle, 0, 1, 2));
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    assert!(reason_of(&reply).contains("not a"), "{}", reason_of(&reply));
}

// ----- logging ----------------------------------------------------------------------------

#[test]
fn log_level_filters_what_reaches_the_host() {
    let t = TestRuntime::with_config(RuntimeConfig {
        log_level: 4,
        ..RuntimeConfig::default()
    });
    // `log_something` warns (level 3), below the configured level 4.
    assert_eq!(
        t.call_sync(function_target(CURRENT_LEVEL_LOG), 1, &[])
            .status,
        ReplyStatus::Ok
    );
    assert!(t.host().take_logs().is_empty());

    let t = TestRuntime::new();
    t.call_sync(function_target(CURRENT_LEVEL_LOG), 1, &[]);
    let logs = t.host().take_logs();
    assert_eq!(logs.len(), 1);
    assert_eq!(
        (logs[0].level, logs[0].message.as_str()),
        (3, "from a free function")
    );
    assert!(
        logs[0].target.contains("common"),
        "default target is the module path: {}",
        logs[0].target
    );
}

// ----- shutdown and stats -----------------------------------------------------------------

#[test]
fn calls_after_shutdown_are_status_5_and_shutdown_is_idempotent() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    assert!(!t.runtime().is_shut_down());
    t.runtime().shutdown();
    t.runtime().shutdown();
    assert!(t.runtime().is_shut_down());
    let reply = call_counter(&t, handle, GET, 2, &[]);
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    assert!(reason_of(&reply).contains("shut down"));
    assert_eq!(t.call(counter_target(handle, GET), 3, &[]), 5);
    assert_eq!(
        t.runtime().objects().live(),
        0,
        "shutdown drops every object"
    );
}

#[test]
fn stats_json_parses_and_tracks_crossings() {
    let t = TestRuntime::new();
    let handle = new_counter(&t, 0, "");
    call_counter(&t, handle, ADD, 2, &enc(&1_i32));
    t.call(counter_target(handle, GET), 3, &[]);
    t.host().script_port_ok(TEST_PORT, ASK, enc(&1_i32));
    t.call(counter_target(handle, ASK_PORT), 4, &enc(&1_i32));
    t.run_pending();

    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    assert_eq!(stats["platform"], "test");
    assert_eq!(stats["mode"], "inproc");
    assert_eq!(stats["live_handles"], 1);
    assert_eq!(stats["live_stores"], 1);
    assert_eq!(stats["tasks"], 0);
    assert_eq!(stats["poisoned_stores"], 0);
    assert_eq!(stats["panics"], 0);
    assert_eq!(stats["crossings"]["calls"], 4);
    assert_eq!(stats["crossings"]["replies"], 4);
    assert_eq!(stats["crossings"]["port_calls"], 1);
    assert_eq!(stats["crossings"]["port_replies"], 1);
    assert!(stats["schema_hash"].as_str().unwrap().starts_with("0x"));
    assert!(
        stats["blocking_threads"]["max"].as_u64().unwrap() >= 1,
        "test runtimes run blocking closures on a real pool (ADR-023)"
    );
    assert_eq!(
        stats["blocking_threads"]["started"], 0,
        "started on demand: nothing ran yet"
    );
}

#[test]
fn stats_json_escapes_the_platform_string() {
    let t = TestRuntime::with_config(RuntimeConfig {
        platform: "we\"ird\n\\plat\u{e9}".into(),
        log_level: 0,
        ..RuntimeConfig::default()
    });
    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    assert_eq!(stats["platform"], "we\"ird\n\\plat\u{e9}");
}

#[test]
fn schema_is_collected_from_registrations() {
    let t = TestRuntime::new();
    let rt = t.runtime();
    assert!(rt.schema().objects.iter().any(|o| o.name == "Counter"));
    assert!(rt.schema().functions.iter().any(|f| f.name == "echo"));
    assert_eq!(rt.schema_hash(), rt.schema().hash());
    let mut w = Writer::new();
    rt.schema_hash().encode(&mut w);
    assert_eq!(w.len(), 8);
}

#[test]
fn invalid_mode_is_rejected() {
    let err = Runtime::new(
        RuntimeConfig {
            mode: "turbo".into(),
            ..RuntimeConfig::default()
        },
        std::sync::Arc::new(undra_runtime::testing::RecordingHost::new()),
    )
    .err()
    .unwrap();
    assert_eq!(err, undra_runtime::InitError::InvalidMode("turbo".into()));
}
