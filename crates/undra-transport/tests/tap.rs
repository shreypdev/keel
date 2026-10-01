//! `ServerConfig::tap`: what `undra dev --record` records from. The tap sees every envelope in
//! both directions, and a recording made from it reads back and replays.
#![cfg(feature = "server")]

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use undra::testing::{EventKind, Recorder, Recording, Target};
use undra::wire::Kind;
use undra::wire::payload::{CallTarget, PortStatus, ReplyStatus};
use undra_transport::{Direction, FrameTap};

type Seen = Arc<Mutex<Vec<(Direction, Kind)>>>;

fn seen_server() -> (Fixture, Seen, Arc<Recorder>) {
    let seen: Seen = Arc::default();
    let recorder = Arc::new(Recorder::new(0, "dev-server"));
    let (log, rec) = (seen.clone(), recorder.clone());
    let config = undra_transport::ServerConfig {
        tap: Some(FrameTap::new(move |direction, kind, payload| {
            log.lock().unwrap().push((direction, kind));
            let _ = rec.record_envelope(kind, payload);
        })),
        ..quick()
    };
    (start_with(config, "dev"), seen, recorder)
}

#[test]
fn the_tap_sees_both_directions_in_order() {
    let (f, seen, _) = seen_server();
    let mut client = f.client();
    let (status, body) = client.call(
        CallTarget::Function { method_id: SUM },
        &[enc(&2_i32), enc(&40_i32)].concat(),
    );
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 42));
    let seen = seen.lock().unwrap().clone();
    let call = seen
        .iter()
        .position(|e| *e == (Direction::HostToCore, Kind::Call))
        .expect("the call");
    let reply = seen
        .iter()
        .position(|e| *e == (Direction::CoreToHost, Kind::Reply))
        .expect("the reply");
    assert!(call < reply);
    // The server's own Hello is an envelope like any other (a recording leaves it out).
    assert!(seen.contains(&(Direction::CoreToHost, Kind::Hello)));
}

#[test]
fn a_recording_made_from_the_tap_reads_back_with_the_session_in_it() {
    let (f, _, recorder) = seen_server();
    let mut client = f.client();
    let handle = client.new_counter(10);
    client.observe(handle, u32::MAX, true);
    let (status, _) = client.method(handle, ADD, &enc(&5_i32));
    assert_eq!(status, ReplyStatus::Ok);
    client.release(handle);
    f.eventually("the release was seen", |_| {
        recorder
            .finish()
            .events
            .iter()
            .any(|e| matches!(e.kind, EventKind::Release { .. }))
    });
    let recording = Recording::from_json(&recorder.finish().to_json()).expect("reads back");
    let kinds: Vec<&str> = recording
        .events
        .iter()
        .map(|e| match &e.kind {
            EventKind::Call {
                target: Target::Constructor { .. },
                ..
            } => "construct",
            EventKind::Call {
                target: Target::Method { .. },
                ..
            } => "method",
            EventKind::Reply { .. } => "reply",
            EventKind::Observe { .. } => "observe",
            EventKind::ChangeSet { .. } => "change_set",
            EventKind::Release { .. } => "release",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds.first(), Some(&"construct"));
    assert!(
        kinds.contains(&"observe") && kinds.contains(&"change_set") && kinds.contains(&"method")
    );
    assert_eq!(kinds.last(), Some(&"release"));
    // Time never goes backwards.
    assert!(recording.events.windows(2).all(|w| w[0].t <= w[1].t));
}

#[test]
fn port_calls_and_the_clients_answers_are_recorded() {
    let (f, _, recorder) = seen_server();
    let mut client = f.client();
    let handle = client.new_counter(0);
    // `ask_later` makes the core call the Echo port (the client answers).
    let call_id = client.next_call_id();
    client.send_call(
        CallTarget::Method {
            handle: undra::wire::Handle(handle),
            method_id: ASK,
        },
        call_id,
        &enc(&7_i32),
    );
    let port_call = client.recv_port_call(ECHO_PORT);
    let echoed =
        undra::wire::payload::PortCall::decode(&mut undra::wire::Reader::new(&port_call.payload))
            .expect("a PortCall");
    client.port_reply(echoed.port_call_id, PortStatus::Ok, &enc(&7_i32));
    f.eventually("the answer was recorded", |_| {
        recorder.finish().events.iter().any(|e| {
            matches!(
                e.kind,
                EventKind::PortReply {
                    status: PortStatus::Ok,
                    ..
                }
            )
        })
    });
    let events = recorder.finish().events;
    let call = events
        .iter()
        .position(|e| matches!(e.kind, EventKind::PortCall { port, .. } if port == ECHO_PORT))
        .expect("the port call");
    let reply = events
        .iter()
        .position(|e| matches!(e.kind, EventKind::PortReply { .. }))
        .expect("the port reply");
    assert!(call < reply);
}

#[test]
fn without_a_tap_nothing_changes() {
    let f = start();
    let mut client = f.client();
    let (status, _) = client.call(
        CallTarget::Function { method_id: SUM },
        &[enc(&1_i32), enc(&1_i32)].concat(),
    );
    assert_eq!(status, ReplyStatus::Ok);
}
