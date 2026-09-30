//! Hostile and broken clients: a bad message ends that one connection with a Close frame and
//! never anything more. Includes a byte-fuzz over the socket.
#![cfg(feature = "server")]

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use common::*;
use keel::runtime::testing::call_payload;
use keel::wire::payload::{CallTarget, ReplyStatus};
use keel::wire::{Envelope, Kind, Writer};
use keel_transport::{ServerConfig, close};

/// The server is up and serves a fresh client, and nothing in it panicked.
fn assert_healthy(f: &Fixture) {
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    let mut client = f.client();
    let (status, body) = client.call(
        CallTarget::Function { method_id: SUM },
        &[enc(&20_i32), enc(&22_i32)].concat(),
    );
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 42));
    assert!(!f.rt.is_shut_down());
    let lines = f.log_lines();
    assert!(
        !lines.iter().any(|l| l.contains("panicked")),
        "something panicked: {lines:?}"
    );
}

/// Sends `bytes` as one binary message after a good handshake and expects a Close.
fn rejected(f: &Fixture, bytes: Vec<u8>) -> (u16, String) {
    let mut client = f.client();
    client.send_binary(bytes);
    let closed = client.expect_close().expect("a Close frame");
    assert_healthy(f);
    closed
}

fn frame(kind: u8, seq: u32, schema: u64, len: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = b"KEEL".to_vec();
    v.extend_from_slice(&1_u16.to_le_bytes());
    v.extend_from_slice(&schema.to_le_bytes());
    v.push(kind);
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(&len.to_le_bytes());
    v.extend_from_slice(payload);
    v
}

// ----- malformed envelopes ------------------------------------------------------------------

#[test]
fn a_malformed_envelope_is_a_1002_close_and_the_server_survives() {
    let f = start();
    let schema = f.schema();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty message", vec![]),
        ("three bytes", vec![1, 2, 3]),
        (
            "bad magic",
            frame(1, 0, schema, 0, &[])
                .into_iter()
                .enumerate()
                .map(|(i, b)| if i == 0 { b'X' } else { b })
                .collect(),
        ),
        ("unsupported version", {
            let mut v = frame(1, 0, schema, 0, &[]);
            v[4] = 9;
            v
        }),
        ("kind 0", frame(0, 0, schema, 0, &[])),
        ("kind 17", frame(17, 0, schema, 0, &[])),
        ("kind 255", frame(255, 0, schema, 0, &[])),
        (
            "truncated header",
            frame(1, 0, schema, 0, &[])[..20].to_vec(),
        ),
        (
            "length larger than the data",
            frame(1, 0, schema, 50, &[1, 2, 3]),
        ),
        (
            "length smaller than the data",
            frame(6, 0, schema, 1, &[1, 0, 0, 0]),
        ),
        ("hostile length", frame(1, 0, schema, u32::MAX, &[])),
    ];
    for (name, bytes) in cases {
        let (code, reason) = rejected(&f, bytes);
        assert_eq!(code, close::PROTOCOL_ERROR, "{name}: {reason}");
        assert!(reason.contains("malformed envelope"), "{name}: {reason}");
    }
}

#[test]
fn a_malformed_payload_for_each_host_bound_kind_is_a_1002_close() {
    let f = start();
    let cases: Vec<(&str, Kind, Vec<u8>)> = vec![
        ("empty call", Kind::Call, vec![]),
        (
            "call with an unknown target",
            Kind::Call,
            vec![9, 0, 0, 0, 0],
        ),
        (
            "call cut before its id",
            Kind::Call,
            vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0],
        ),
        ("short cancel", Kind::Cancel, vec![1, 2]),
        (
            "cancel with trailing bytes",
            Kind::Cancel,
            vec![1, 0, 0, 0, 9],
        ),
        ("short credit", Kind::StreamCredit, vec![1, 0, 0, 0]),
        ("observe with on = 2", Kind::Observe, {
            let mut v = enc(&1_u64);
            v.extend(enc(&0_u32));
            v.push(2);
            v
        }),
        ("short observe", Kind::Observe, vec![1; 12]),
        ("empty release", Kind::Release, vec![]),
        ("short event", Kind::Event, vec![1, 2, 3]),
        ("short timer", Kind::TimerFired, vec![1]),
        (
            "port reply with an unknown status",
            Kind::PortReply,
            vec![1, 0, 0, 0, 7],
        ),
        ("empty port reply", Kind::PortReply, vec![]),
    ];
    for (name, kind, payload) in cases {
        let mut client = f.client();
        client.send(kind, &payload);
        let (code, reason) = client
            .expect_close()
            .unwrap_or_else(|| panic!("{name}: no Close frame"));
        assert_eq!(code, close::PROTOCOL_ERROR, "{name}: {reason}");
        assert!(reason.contains("malformed"), "{name}: {reason}");
        assert_healthy(&f);
    }
}

#[test]
fn well_formed_but_meaningless_messages_are_harmless() {
    // Unknown handles, ids nobody opened, ports nobody called: the runtime ignores them and the
    // connection stays open; core-to-host kinds sent by a client are ignored too.
    let f = start();
    let mut client = f.client();
    let mut rng = Rng(7);
    for _ in 0..300 {
        let payload = match rng.below(7) {
            0 => {
                let mut w = Writer::new();
                w.write_u64(rng.next());
                w.write_u32(rng.next() as u32);
                w.write_u8((rng.next() & 1) as u8);
                (Kind::Observe, w.into_vec())
            }
            1 => (Kind::Release, rng.next().to_le_bytes().to_vec()),
            2 => (Kind::Cancel, (rng.next() as u32).to_le_bytes().to_vec()),
            3 => {
                let mut v = (rng.next() as u32).to_le_bytes().to_vec();
                v.extend((rng.next() as u32).to_le_bytes());
                (Kind::StreamCredit, v)
            }
            4 => {
                let mut v = (rng.next() as u32).to_le_bytes().to_vec();
                v.extend((rng.next() as u32).to_le_bytes());
                v.extend(rng.junk(20));
                (Kind::Event, v)
            }
            5 => (Kind::TimerFired, (rng.next() as u32).to_le_bytes().to_vec()),
            _ => {
                let misdirected = [
                    Kind::Reply,
                    Kind::ChangeSet,
                    Kind::PortCall,
                    Kind::StreamItem,
                    Kind::Log,
                    Kind::Snapshot,
                ];
                (misdirected[rng.below(misdirected.len())], rng.junk(30))
            }
        };
        client.send(payload.0, &payload.1);
    }
    // Still connected and still served.
    let (status, _) = client.call(
        CallTarget::Function { method_id: SUM },
        &[enc(&1_i32), enc(&1_i32)].concat(),
    );
    assert_eq!(status, ReplyStatus::Ok);
    assert!(f.bridge.is_connected());
}

#[test]
fn a_text_message_is_unsupported_data() {
    let f = start();
    let mut client = f.client();
    client.send_text("hello");
    let (code, reason) = client.expect_close().expect("a Close frame");
    assert_eq!(code, close::UNSUPPORTED_DATA);
    assert!(reason.contains("binary"), "{reason}");
    assert_healthy(&f);
}

#[test]
fn a_message_over_the_limit_is_closed_with_message_too_big() {
    let config = ServerConfig {
        max_message_bytes: 4096,
        ..quick()
    };
    let f = start_with(config, "dev");
    let mut client = f.client();
    let big = call_payload(
        CallTarget::Function {
            method_id: ECHO_BYTES,
        },
        1,
        &vec![0_u8; 10_000],
    );
    let mut w = Writer::new();
    Envelope::write(&mut w, Kind::Call, 1, f.schema(), &big);
    let _ = client.ws().send(tungstenite::Message::Binary(w.into_vec()));
    let (code, _) = client.expect_close().expect("a Close frame");
    assert_eq!(code, close::MESSAGE_TOO_BIG);
    assert_healthy(&f);
}

#[test]
fn a_mid_session_error_still_cancels_what_the_client_had_started() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    client.observe(handle, u32::MAX, true);
    client.send_binary(vec![0xff; 40]);
    client.expect_close().expect("a Close frame");
    f.eventually("its objects were released", |f| {
        stat(&f.rt, "live_handles") == 0
    });
    assert_healthy(&f);
}

// ----- broken WebSocket framing -------------------------------------------------------------

#[test]
fn websocket_protocol_violations_are_1002_closes() {
    let f = start();
    let addr = f.server.addr();
    let hello = {
        let mut w = Writer::new();
        keel::wire::payload::Hello {
            keel_version: "t",
            schema_hash: f.schema(),
            platform: "raw",
            mode: "dev",
        }
        .encode(&mut w);
        let mut e = Writer::new();
        Envelope::write(&mut e, Kind::Hello, 0, f.schema(), w.as_slice());
        e.into_vec()
    };
    type Break = fn(&mut RawWs);
    let cases: Vec<(&str, Break)> = vec![
        ("an unmasked frame", |ws| ws.frame(0x82, &[1, 2, 3], false)),
        ("reserved bits", |ws| ws.frame(0xc2, &[1, 2, 3], true)),
        ("a reserved opcode", |ws| ws.frame(0x83, &[1], true)),
        ("a fragmented control frame", |ws| ws.frame(0x09, &[], true)),
        ("an oversized ping", |ws| ws.frame(0x89, &[0; 126], true)),
        ("a continuation with nothing to continue", |ws| {
            ws.frame(0x80, &[1], true)
        }),
    ];
    for (name, break_it) in cases {
        let mut ws = RawWs::connect(addr);
        ws.binary(&hello); // a session, so the break happens after the handshake
        break_it(&mut ws);
        let frames = ws.frames_until_end(Duration::from_secs(3));
        assert_eq!(
            RawWs::close_code(&frames),
            Some(close::PROTOCOL_ERROR),
            "{name}: {frames:?}"
        );
        assert_healthy(&f);
    }
}

#[test]
fn a_text_message_with_invalid_utf8_is_closed_with_invalid_payload() {
    let f = start();
    let mut ws = RawWs::connect(f.server.addr());
    ws.frame(0x81, &[0xff, 0xfe, 0xfd], true);
    let frames = ws.frames_until_end(Duration::from_secs(3));
    assert_eq!(
        RawWs::close_code(&frames),
        Some(close::INVALID_PAYLOAD),
        "{frames:?}"
    );
    assert_healthy(&f);
}

#[test]
fn a_ping_is_answered_with_a_pong_carrying_the_same_bytes_even_between_fragments() {
    let f = start();
    let mut ws = RawWs::connect(f.server.addr());
    ws.frame(0x89, b"are you there", true);
    // A Hello split over three fragments with a ping in the middle.
    let hello = {
        let mut w = Writer::new();
        keel::wire::payload::Hello {
            keel_version: "t",
            schema_hash: f.schema(),
            platform: "raw",
            mode: "dev",
        }
        .encode(&mut w);
        let mut e = Writer::new();
        Envelope::write(&mut e, Kind::Hello, 0, f.schema(), w.as_slice());
        e.into_vec()
    };
    let (a, rest) = hello.split_at(10);
    let (b, c) = rest.split_at(15);
    ws.frame(0x02, a, true); // binary, not final
    ws.frame(0x89, b"mid", true);
    ws.frame(0x00, b, true); // continuation
    ws.frame(0x80, c, true); // final continuation
    ws.tcp
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let frames = ws.frames_until_end(Duration::from_millis(700));
    let pongs: Vec<&[u8]> = frames
        .iter()
        .filter(|f| f.opcode == 10)
        .map(|f| f.payload.as_slice())
        .collect();
    assert_eq!(pongs, [&b"are you there"[..], &b"mid"[..]]);
    assert!(
        frames.iter().any(|f| f.opcode == 2),
        "the fragmented Hello was reassembled and answered: {frames:?}"
    );
    drop(ws);
    assert_healthy(&f);
}

#[test]
fn something_that_is_not_a_websocket_client_gets_nothing_and_the_server_carries_on() {
    let f = start();
    let addr = f.server.addr();
    // Plain HTTP without the upgrade headers.
    let mut http = RawWs::connect_socket(addr);
    http.tcp
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut answer = Vec::new();
    let _ = http.tcp.read_to_end(&mut answer);
    assert!(!String::from_utf8_lossy(&answer).starts_with("HTTP/1.1 101"));
    // Binary junk, and a request cut off mid-header.
    for junk in [
        &b"\x00\x01\x02\x03garbage"[..],
        &b"GET / HTT"[..],
        &[0xff; 4096][..],
        b"\r\n\r\n",
    ] {
        let mut socket = TcpStream::connect(addr).unwrap();
        let _ = socket.write_all(junk);
    }
    assert_healthy(&f);
}

#[test]
fn a_connection_that_stalls_in_the_upgrade_is_dropped() {
    let config = ServerConfig {
        handshake_timeout: Duration::from_millis(200),
        ..quick()
    };
    let f = start_with(config, "dev");
    let mut socket = TcpStream::connect(f.server.addr()).unwrap();
    socket.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n").unwrap(); // never finishes
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut buf = [0_u8; 64];
    assert!(matches!(socket.read(&mut buf), Ok(0) | Err(_)));
    assert_healthy(&f);
}

// ----- byte fuzz ------------------------------------------------------------------------------

#[test]
fn byte_fuzz_before_the_upgrade_never_hurts_the_server() {
    let f = start();
    let addr = f.server.addr();
    let mut rng = Rng(0x5eed);
    for i in 0..150 {
        let mut bytes = if i % 3 == 0 {
            b"GET / HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n".to_vec()
        } else {
            Vec::new()
        };
        bytes.extend(rng.junk(400));
        if let Ok(mut socket) = TcpStream::connect(addr) {
            let _ = socket.write_all(&bytes);
        }
    }
    assert_healthy(&f);
}

#[test]
fn byte_fuzz_after_the_handshake_never_panics_the_server() {
    let f = start();
    let mut rng = Rng(0xfeed_beef);
    for _ in 0..80 {
        let mut client = f.client();
        for _ in 0..4 {
            if client
                .ws()
                .send(tungstenite::Message::Binary(rng.junk(200)))
                .is_err()
            {
                break;
            }
        }
        // Whatever it sent, the server either closed it or ignored it; either way it is done.
        drop(client);
        f.eventually("the slot is free", |f| !f.bridge.is_connected());
    }
    assert_healthy(&f);
}

#[test]
fn mutated_valid_frames_never_panic_the_server() {
    let f = start();
    let schema = f.schema();
    let mut rng = Rng(0xc0ffee);
    let bases: Vec<Vec<u8>> = {
        let make = |kind: Kind, payload: Vec<u8>| {
            let mut w = Writer::new();
            Envelope::write(&mut w, kind, 1, schema, &payload);
            w.into_vec()
        };
        let mut observe = enc(&0x0000_0001_0000_0001_u64);
        observe.extend(enc(&u32::MAX));
        observe.push(1);
        vec![
            make(
                Kind::Call,
                call_payload(
                    CallTarget::Function { method_id: SUM },
                    1,
                    &[enc(&1_i32), enc(&2_i32)].concat(),
                ),
            ),
            make(
                Kind::Call,
                call_payload(
                    CallTarget::Constructor {
                        type_id: COUNTER,
                        method_id: NEW,
                    },
                    2,
                    &enc(&5_i32),
                ),
            ),
            make(
                Kind::Call,
                call_payload(
                    CallTarget::Method {
                        handle: keel::wire::Handle(0x0000_0001_0000_0001),
                        method_id: ADD,
                    },
                    3,
                    &enc(&1_i32),
                ),
            ),
            make(Kind::Observe, observe),
            make(Kind::Release, enc(&0x0000_0001_0000_0001_u64)),
            make(Kind::Cancel, enc(&1_u32)),
            make(Kind::PortReply, [enc(&1_u32), vec![0]].concat()),
            make(Kind::Restore, vec![0; 8]),
            make(Kind::Event, [enc(&1_u32), enc(&2_u32), vec![1]].concat()),
        ]
    };
    let mut mutants = 0;
    for _ in 0..150 {
        let mut bytes = bases[rng.below(bases.len())].clone();
        match rng.below(4) {
            0 => {
                for _ in 0..=rng.below(3) {
                    let at = rng.below(bytes.len());
                    bytes[at] ^= (rng.next() as u8) | 1;
                }
            }
            1 => bytes.truncate(rng.below(bytes.len())),
            2 => {
                bytes.extend(rng.junk(16));
                bytes.push(0);
            }
            _ => {
                let at = rng.below(bytes.len());
                bytes[at] = rng.next() as u8;
            }
        }
        let mut client = f.client();
        client.send_binary(bytes);
        // The mutant either did nothing (still connected) or ended the session.
        let _ = client.recv_within(Duration::from_millis(20));
        drop(client);
        f.eventually("the slot is free", |f| !f.bridge.is_connected());
        mutants += 1;
    }
    assert_eq!(mutants, 150);
    assert_healthy(&f);
}
