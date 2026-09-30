//! Streams: open, credit, items, end and cancel over the socket (SPEC 3.7).
#![cfg(feature = "server")]

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use common::*;
use keel::wire::payload::{CallTarget, ReplyStatus, StreamFlag};
use keel::wire::{Handle, Kind};

fn open(client: &mut TestClient, handle: u64, method_id: u32, args: &[u8]) -> u32 {
    let id = client.next_call_id();
    client.send_call(
        CallTarget::Method {
            handle: Handle(handle),
            method_id,
        },
        id,
        args,
    );
    let (status, body) = client.await_reply(id);
    assert_eq!(status, ReplyStatus::StreamOpened);
    assert!(body.is_empty());
    id
}

fn next_item(client: &mut TestClient) -> (u32, StreamFlag, Vec<u8>) {
    stream_item(&client.recv_kind(Kind::StreamItem))
}

#[test]
fn a_stream_sends_nothing_until_credited_and_then_only_what_was_credited() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let id = open(&mut client, handle, TICKS, &enc(&5_u32));

    assert!(
        client.silent_for(Duration::from_millis(150)),
        "the initial credit is 0: no item may arrive"
    );
    client.credit(id, 2);
    assert_eq!(next_item(&mut client), (id, StreamFlag::Item, enc(&0_u32)));
    assert_eq!(next_item(&mut client), (id, StreamFlag::Item, enc(&1_u32)));
    assert!(
        client.silent_for(Duration::from_millis(150)),
        "only two were credited"
    );

    client.credit(id, 16);
    for n in 2..5_u32 {
        assert_eq!(next_item(&mut client), (id, StreamFlag::Item, enc(&n)));
    }
    assert_eq!(next_item(&mut client), (id, StreamFlag::End, Vec::new()));
}

#[test]
fn stream_items_carry_gapless_sequence_numbers_and_the_core_hash() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let id = open(&mut client, handle, TICKS, &enc(&50_u32));
    client.credit(id, 100);
    for _ in 0..50 {
        assert_eq!(next_item(&mut client).1, StreamFlag::Item);
    }
    assert_eq!(next_item(&mut client).1, StreamFlag::End);
    let seqs: Vec<u32> = client.seen.iter().map(|frame| frame.seq).collect();
    assert_eq!(seqs, (0..seqs.len() as u32).collect::<Vec<_>>());
    assert!(client.seen.iter().all(|frame| frame.schema == f.schema()));
}

#[test]
fn cancelling_a_stream_closes_it_without_a_reply_and_stops_the_items() {
    let _serial = serial();
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let before = DROPPED.load(Ordering::SeqCst);
    let id = open(&mut client, handle, ENDLESS, &[]);
    client.credit(id, 3);
    for n in 0..3_u32 {
        assert_eq!(next_item(&mut client), (id, StreamFlag::Item, enc(&n)));
    }
    client.cancel(id);
    eventually("the stream was dropped", || {
        DROPPED.load(Ordering::SeqCst) > before
    });
    client.credit(id, 100); // too late: nothing left to send
    assert!(
        client.silent_for(Duration::from_millis(200)),
        "no reply and no items after a cancel: {:?}",
        client.seen.iter().map(|f| f.kind).collect::<Vec<_>>()
    );
}

#[test]
fn two_streams_share_the_connection_independently() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let a = open(&mut client, handle, TICKS, &enc(&3_u32));
    let b = open(&mut client, handle, TICKS, &enc(&3_u32));
    client.credit(a, 1);
    client.credit(b, 3);
    let mut seen = Vec::new();
    // b: 0,1,2 (and then its End needs credit? no: End is not credited); a: 0
    while !(seen.contains(&(b, StreamFlag::End))
        && seen.iter().filter(|(id, _)| *id == a).count() == 1)
    {
        let (id, flag, _) = next_item(&mut client);
        seen.push((id, flag));
    }
    assert_eq!(
        seen.iter()
            .filter(|(id, f)| *id == b && *f == StreamFlag::Item)
            .count(),
        3
    );
    assert_eq!(
        seen.iter()
            .filter(|(id, f)| *id == a && *f == StreamFlag::Item)
            .count(),
        1
    );
}

#[test]
fn a_stream_that_ended_frees_its_call_id_for_the_next_call() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let id = open(&mut client, handle, TICKS, &enc(&1_u32));
    client.credit(id, 4);
    assert_eq!(next_item(&mut client).1, StreamFlag::Item);
    assert_eq!(next_item(&mut client).1, StreamFlag::End);
    // The same id again, as a plain call: answered, not ignored as a duplicate.
    client.send_call(
        CallTarget::Method {
            handle: Handle(handle),
            method_id: GET,
        },
        id,
        &[],
    );
    let (status, body) = client.await_reply(id);
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 0));
}
