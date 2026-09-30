//! Calls and replies over a real socket: the paths the platform runtimes exercise against
//! their fake servers, here against the real runtime.
#![cfg(feature = "server")]

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use common::*;
use keel::wire::payload::{CallTarget, ReplyStatus};
use keel::wire::{Bytes, Handle, Kind};

fn function(method_id: u32) -> CallTarget {
    CallTarget::Function { method_id }
}

fn sum_args(a: i32, b: i32) -> Vec<u8> {
    [enc(&a), enc(&b)].concat()
}

#[test]
fn a_sync_call_is_answered_with_its_reply() {
    let f = start();
    let mut client = f.client();
    let (status, body) = client.call(function(SUM), &sum_args(2, 40));
    assert_eq!(status, ReplyStatus::Ok);
    assert_eq!(dec::<i32>(&body), 42);
}

#[test]
fn constructors_methods_and_typed_errors() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(10);
    assert_ne!(handle, 0);

    let (status, body) = client.method(handle, ADD, &enc(&5_i32));
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 15));
    let (status, body) = client.method(handle, GET, &[]);
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 15));

    let (status, body) = client.method(handle, CHECK, &enc(&7_i32));
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 7));
    let (status, body) = client.method(handle, CHECK, &enc(&-1_i32));
    assert_eq!(status, ReplyStatus::Error, "a typed error is status 1");
    assert_eq!(dec::<CounterError>(&body), CounterError::Negative(-1));
}

#[test]
fn an_async_call_replies_when_it_finishes() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(1);
    let (status, body) = client.method(handle, SLOW_ADD, &enc(&4_i32));
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 5));
}

#[test]
fn replies_to_concurrent_calls_arrive_in_completion_order_and_match_their_ids() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    // The slow call is sent first and finishes last.
    let slow = client.next_call_id();
    client.send_call(
        CallTarget::Method {
            handle: Handle(handle),
            method_id: SLOW_ADD,
        },
        slow,
        &enc(&100_i32),
    );
    let fast = client.next_call_id();
    client.send_call(
        CallTarget::Method {
            handle: Handle(handle),
            method_id: ADD,
        },
        fast,
        &enc(&1_i32),
    );
    let (fast_status, fast_body) = client.await_reply(fast);
    assert_eq!((fast_status, dec::<i32>(&fast_body)), (ReplyStatus::Ok, 1));
    let (slow_status, slow_body) = client.await_reply(slow);
    assert_eq!(
        (slow_status, dec::<i32>(&slow_body)),
        (ReplyStatus::Ok, 101)
    );
}

#[test]
fn a_panic_in_the_core_is_a_status_2_reply_and_the_server_carries_on() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let (status, body) = client.method(handle, BOOM, &[]);
    assert_eq!(status, ReplyStatus::Panic);
    let message = dec::<(String, String)>(&body).0;
    assert!(message.contains("kaboom"), "{message}");
    let (status, _) = client.method(handle, GET, &[]);
    assert_eq!(status, ReplyStatus::Ok);
}

#[test]
fn unknown_methods_and_undecodable_arguments_are_status_5() {
    let f = start();
    let mut client = f.client();
    let (status, body) = client.call(function(0xdead_0001), &[]);
    assert_eq!(status, ReplyStatus::BadRequest);
    assert!(!dec::<String>(&body).is_empty());
    let (status, _) = client.call(function(SUM), &[1, 2, 3]);
    assert_eq!(status, ReplyStatus::BadRequest);
    let (status, _) = client.method(0x0000_0001_0000_0099, GET, &[]);
    assert_eq!(status, ReplyStatus::BadRequest, "a stale handle");
}

#[test]
fn call_id_zero_is_answered_not_dropped() {
    let f = start();
    let mut client = f.client();
    client.send_call(function(SUM), 0, &sum_args(1, 1));
    let (status, body) = client.await_reply(0);
    assert_eq!(status, ReplyStatus::BadRequest);
    assert!(dec::<String>(&body).contains("refused"));
}

#[test]
fn a_call_id_that_is_already_open_is_ignored_and_the_first_call_still_replies() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let id = client.next_call_id();
    let target = CallTarget::Method {
        handle: Handle(handle),
        method_id: SLOW_ADD,
    };
    client.send_call(target, id, &enc(&1_i32));
    client.send_call(target, id, &enc(&1_i32));
    let (status, body) = client.await_reply(id);
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 1));
    // One reply only, and the counter moved once.
    let (_, body) = client.method(handle, GET, &[]);
    assert_eq!(dec::<i32>(&body), 1);
    assert_eq!(
        client.frames_of(Kind::Reply).len(),
        3,
        "constructor, one slow_add, get"
    );
}

#[test]
fn cancelling_an_async_call_drops_it_and_replies_status_3() {
    let _serial = serial();
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let before = DROPPED.load(Ordering::SeqCst);
    let id = client.next_call_id();
    client.send_call(
        CallTarget::Method {
            handle: Handle(handle),
            method_id: HANG,
        },
        id,
        &[],
    );
    eventually("the call started", || STARTED.load(Ordering::SeqCst) > 0);
    client.cancel(id);
    let (status, body) = client.await_reply(id);
    assert_eq!(status, ReplyStatus::Cancelled);
    assert!(body.is_empty());
    eventually("the future was dropped", || {
        DROPPED.load(Ordering::SeqCst) > before
    });
}

#[test]
fn large_arguments_and_replies_survive_the_framing() {
    let f = start();
    let mut client = f.client();
    // Bigger than a 16-bit frame length and than the default write buffer.
    let data: Vec<u8> = (0..3_000_000_u32).map(|i| (i * 7) as u8).collect();
    let (status, body) = client.call(function(ECHO_BYTES), &enc(&Bytes(data.clone())));
    assert_eq!(status, ReplyStatus::Ok);
    assert_eq!(dec::<Bytes>(&body).0, data);
}

#[test]
fn many_calls_in_flight_are_all_answered_with_gapless_server_sequence_numbers() {
    let f = start();
    let mut client = f.client();
    const N: u32 = 300;
    for i in 1..=N {
        client.send_call(function(SUM), i, &sum_args(i as i32, 1));
    }
    let mut answered = vec![false; N as usize + 1];
    for _ in 0..N {
        let frame = client.recv_kind(Kind::Reply);
        let reply =
            keel::wire::payload::Reply::decode(&mut keel::wire::Reader::new(&frame.payload))
                .unwrap();
        assert_eq!(reply.status, ReplyStatus::Ok);
        assert_eq!(dec::<i32>(reply.body), reply.call_id as i32 + 1);
        answered[reply.call_id as usize] = true;
    }
    assert!(answered[1..].iter().all(|&a| a));
    let seqs: Vec<u32> = client.seen.iter().map(|frame| frame.seq).collect();
    assert_eq!(seqs, (0..seqs.len() as u32).collect::<Vec<_>>());
}

#[test]
fn a_reply_written_from_a_core_thread_reaches_a_client_that_is_busy_reading_nothing() {
    // The core answers an async call while the client is not reading; the frame waits in the
    // queue and arrives later, intact.
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let id = client.next_call_id();
    client.send_call(
        CallTarget::Method {
            handle: Handle(handle),
            method_id: SLOW_ADD,
        },
        id,
        &enc(&9_i32),
    );
    std::thread::sleep(Duration::from_millis(150));
    let (status, body) = client.await_reply(id);
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 9));
}
