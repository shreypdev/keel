//! The Hello exchange, as the TypeScript, Kotlin and Swift `remote` transports perform it.
#![cfg(feature = "server")]

mod common;

use std::time::Duration;

use common::*;
use keel::wire::payload::{Hello, ReplyStatus};
use keel::wire::{Kind, Reader};
use keel_transport::{ClientInfo, KEEL_VERSION, ServerConfig, close};

#[test]
fn the_server_answers_the_clients_hello_with_its_own_as_sequence_zero() {
    let f = start();
    let mut client = TestClient::connect_raw(&f.url(), f.schema());
    client.send_hello(f.schema(), "ios", "dev");
    let frame = client.expect_frame(Kind::Hello);

    assert_eq!(frame.seq, 0, "the server's first envelope is sequence 0");
    assert_eq!(
        frame.schema,
        f.schema(),
        "the header carries the core's schema hash"
    );
    let hello = Hello::decode(&mut Reader::new(&frame.payload)).unwrap();
    assert_eq!(hello.keel_version, KEEL_VERSION);
    assert_eq!(hello.schema_hash, f.schema());
    assert_eq!(hello.platform, "rust");
    assert_eq!(hello.mode, "dev");

    f.eventually("the bridge sees the client", |f| f.bridge.is_connected());
    assert_eq!(
        f.bridge.client(),
        Some(ClientInfo {
            keel_version: "0.0.0-test".into(),
            platform: "ios".into(),
            mode: "dev".into(),
        })
    );
}

#[test]
fn a_schema_mismatch_is_reported_from_the_servers_hello_and_then_closed() {
    let f = start();
    let wrong = f.schema() ^ 0xdead_beef;
    let mut client = TestClient::connect_raw(&f.url(), wrong);
    client.send_hello(wrong, "web", "dev");

    // The client reads the mismatch from this message: the core's hash, not the client's.
    let frame = client.expect_frame(Kind::Hello);
    let hello = Hello::decode(&mut Reader::new(&frame.payload)).unwrap();
    assert_eq!(hello.schema_hash, f.schema());
    assert_ne!(hello.schema_hash, wrong);

    let (code, reason) = client.expect_close().expect("a Close frame");
    assert_eq!(code, close::POLICY_VIOLATION);
    assert!(reason.contains("schema mismatch"), "{reason}");
    assert!(
        reason.contains(&format!("{:#018x}", f.schema())),
        "{reason}"
    );
    assert!(
        !f.bridge.is_connected(),
        "a refused client is never attached"
    );

    // The server carries on: a client with the right hash is served.
    let mut good = f.client();
    assert_eq!(good.new_counter(3) > 0, true);
}

#[test]
fn a_client_that_says_nothing_is_closed_when_the_handshake_times_out() {
    let config = ServerConfig {
        handshake_timeout: Duration::from_millis(250),
        ..quick()
    };
    let f = start_with(config, "dev");
    let mut client = TestClient::connect_raw(&f.url(), f.schema());
    let (code, reason) = client.expect_close().expect("a Close frame");
    assert_eq!(code, close::POLICY_VIOLATION);
    assert!(reason.contains("no Hello"), "{reason}");
    assert!(!f.bridge.is_connected());
}

#[test]
fn the_first_message_must_be_hello() {
    let f = start();
    let mut client = TestClient::connect_raw(&f.url(), f.schema());
    client.send_call(
        keel::wire::payload::CallTarget::Function { method_id: SUM },
        1,
        &[],
    );
    let (code, reason) = client.expect_close().expect("a Close frame");
    assert_eq!(code, close::PROTOCOL_ERROR);
    assert!(reason.contains("expected Hello"), "{reason}");
    assert!(
        client.frames_of(Kind::Reply).is_empty(),
        "the call was never run"
    );
}

#[test]
fn a_malformed_hello_payload_is_a_protocol_error() {
    let f = start();
    let mut client = TestClient::connect_raw(&f.url(), f.schema());
    client.send(Kind::Hello, &[1, 2, 3]);
    let (code, reason) = client.expect_close().expect("a Close frame");
    assert_eq!(code, close::PROTOCOL_ERROR);
    assert!(reason.contains("malformed Hello"), "{reason}");
}

#[test]
fn client_sequence_numbers_are_not_validated() {
    // TypeScript and Kotlin count from 0, Swift from 1 (its first envelope, the Hello, is 1).
    for start_at in [0, 1, 1_000_000] {
        let f = start();
        let mut client = TestClient::connect_raw(&f.url(), f.schema());
        client.start_seq_at(start_at);
        client.handshake("swift", "dev");
        let (status, body) = client.call(
            keel::wire::payload::CallTarget::Function { method_id: SUM },
            &[enc(&2_i32), enc(&40_i32)].concat(),
        );
        assert_eq!(status, ReplyStatus::Ok);
        assert_eq!(dec::<i32>(&body), 42);
    }
}

#[test]
fn a_repeated_hello_is_ignored() {
    let f = start();
    let mut client = f.client();
    client.send_hello(f.schema(), "test", "dev");
    let (status, _) = client.call(
        keel::wire::payload::CallTarget::Function { method_id: SUM },
        &[enc(&1_i32), enc(&1_i32)].concat(),
    );
    assert_eq!(status, ReplyStatus::Ok);
    assert!(
        client.frames_of(Kind::Hello).len() == 1,
        "the server did not answer twice"
    );
}

#[test]
fn an_envelope_with_another_schema_hash_after_the_handshake_ends_the_session() {
    let f = start();
    let mut client = f.client();
    client.schema ^= 0x99;
    client.send(Kind::Release, &enc(&0_u64));
    let (code, reason) = client.expect_close().expect("a Close frame");
    assert_eq!(code, close::POLICY_VIOLATION);
    assert!(reason.contains("schema mismatch"), "{reason}");
    f.eventually("the slot is free again", |f| !f.bridge.is_connected());
}

#[test]
fn every_envelope_the_server_sends_carries_the_core_hash_and_gapless_sequence_numbers() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    client.observe(handle, u32::MAX, true);
    for i in 1..=5 {
        client.method(handle, ADD, &enc(&i));
    }
    let seqs: Vec<u32> = client.seen.iter().map(|frame| frame.seq).collect();
    assert!(seqs.len() >= 12, "hello, 2+ replies, change-sets: {seqs:?}");
    assert_eq!(seqs, (0..seqs.len() as u32).collect::<Vec<_>>());
    assert!(client.seen.iter().all(|frame| frame.schema == f.schema()));
}
