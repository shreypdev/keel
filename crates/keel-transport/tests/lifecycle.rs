//! Connection lifecycle: what a disconnect cleans up, the one-client policy, shutdown.
#![cfg(feature = "server")]

mod common;

use std::io::Read;
use std::net::TcpStream;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use keel::runtime::testing::call_payload;
use keel::runtime::{InitError, Runtime, RuntimeConfig};
use keel::wire::payload::{CallTarget, ReplyStatus};
use keel::wire::{Bytes, Handle, Kind};
use keel_transport::{ServeError, Server, ServerConfig, close};
use tungstenite::Message;
use tungstenite::protocol::CloseFrame;
use tungstenite::protocol::frame::coding::CloseCode;

// ----- what a disconnect cleans up -----------------------------------------------------------

#[test]
fn a_disconnect_cancels_the_calls_in_flight() {
    let _serial = serial();
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let started = STARTED.load(Ordering::SeqCst);
    let dropped = DROPPED.load(Ordering::SeqCst);
    let id = client.next_call_id();
    client.send_call(
        CallTarget::Method {
            handle: Handle(handle),
            method_id: HANG,
        },
        id,
        &[],
    );
    eventually("the call started", || {
        STARTED.load(Ordering::SeqCst) > started
    });
    assert_eq!(stat(&f.rt, "active_calls"), 1);

    drop(client); // no Close frame, no cancel: the socket just goes away

    eventually("its future was dropped", || {
        DROPPED.load(Ordering::SeqCst) > dropped
    });
    f.eventually("no call is left", |f| stat(&f.rt, "active_calls") == 0);
    assert_eq!(stat(&f.rt, "cancelled"), 1);
}

#[test]
fn a_disconnect_cancels_open_streams() {
    let _serial = serial();
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let dropped = DROPPED.load(Ordering::SeqCst);
    let id = client.next_call_id();
    client.send_call(
        CallTarget::Method {
            handle: Handle(handle),
            method_id: ENDLESS,
        },
        id,
        &[],
    );
    client.await_reply(id);
    assert_eq!(stat(&f.rt, "open_streams"), 1);
    drop(client);
    eventually("the stream was dropped", || {
        DROPPED.load(Ordering::SeqCst) > dropped
    });
    f.eventually("no stream is left", |f| stat(&f.rt, "open_streams") == 0);
}

#[test]
fn a_disconnect_releases_the_objects_the_clients_constructors_made() {
    let f = start();
    let mut client = f.client();
    for n in 0..3 {
        let handle = client.new_counter(n);
        client.observe(handle, u32::MAX, true);
    }
    assert_eq!(stat(&f.rt, "live_handles"), 3);
    drop(client);
    f.eventually("its objects are gone", |f| stat(&f.rt, "live_handles") == 0);
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
}

#[test]
fn objects_the_client_released_itself_are_not_released_twice() {
    let f = start();
    let mut client = f.client();
    let a = client.new_counter(1);
    let _b = client.new_counter(2);
    client.release(a);
    // A round trip proves the release was processed.
    client.new_counter(3);
    assert_eq!(stat(&f.rt, "live_handles"), 2);
    drop(client);
    f.eventually("all released", |f| stat(&f.rt, "live_handles") == 0);
    assert!(
        !f.log_lines().iter().any(|line| line.contains("release:")),
        "no stale release was attempted: {:?}",
        f.log_lines()
    );
}

#[test]
fn objects_survive_a_disconnect_when_release_on_disconnect_is_off_but_observation_ends() {
    let config = ServerConfig {
        release_on_disconnect: false,
        ..quick()
    };
    let f = start_with(config, "dev");
    let mut client = f.client();
    let handle = client.new_counter(0);
    client.observe(handle, u32::MAX, true);
    client.method(handle, GET, &[]); // a round trip: the observation is in place
    let before = stat(&f.rt, "change_sets");
    drop(client);
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    assert_eq!(stat(&f.rt, "live_handles"), 1, "the object is still there");

    // Nobody observes it any more: a write produces no change-set.
    let counter = f.rt.object::<Counter>(handle).expect("still live");
    on_core(&f.rt, move || counter.add(1));
    assert_eq!(
        stat(&f.rt, "change_sets"),
        before,
        "observation ended with the connection"
    );
}

#[test]
fn the_server_notes_connections_and_disconnections() {
    let f = start();
    let client = f.client();
    f.eventually("a connect note", |f| {
        f.log_lines()
            .iter()
            .any(|l| l.contains("client connected: platform=test mode=dev"))
    });
    drop(client);
    f.eventually("a disconnect note", |f| {
        f.log_lines()
            .iter()
            .any(|l| l.contains("client disconnected"))
    });
}

// ----- one client at a time ------------------------------------------------------------------

#[test]
fn a_second_client_gets_the_servers_hello_and_is_closed_with_try_again_later() {
    let f = start();
    let mut first = f.client();
    let handle = first.new_counter(1);

    let mut second = TestClient::connect_raw(&f.url(), f.schema());
    second.send_hello(f.schema(), "android", "dev");
    second.expect_frame(Kind::Hello);
    let (code, reason) = second.expect_close().expect("a Close frame");
    assert_eq!(code, close::TRY_AGAIN_LATER);
    assert!(
        reason.contains("another client is already connected"),
        "{reason}"
    );

    // The first client is untouched.
    let (status, body) = first.method(handle, GET, &[]);
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 1));
    assert_eq!(f.bridge.client().unwrap().platform, "test");
}

#[test]
fn a_reconnect_right_after_a_disconnect_wins_the_race_for_the_slot() {
    // A page reload or an app relaunch: the new socket is up before the old one is torn down.
    let f = start();
    for round in 0..15 {
        let mut client = f.client();
        let handle = client.new_counter(round);
        drop(client);
        let mut next = f.client(); // no waiting in between
        let (status, _) = next.method(handle, GET, &[]);
        assert_eq!(
            status,
            ReplyStatus::BadRequest,
            "the old session's handle is gone"
        );
        let fresh = next.new_counter(round);
        assert_ne!(fresh, 0);
    }
}

#[test]
fn a_clean_close_from_the_client_is_echoed_and_frees_the_slot() {
    let f = start();
    let mut client = f.client();
    client.new_counter(0);
    client
        .ws()
        .close(Some(CloseFrame {
            code: CloseCode::Normal,
            reason: "bye".into(),
        }))
        .unwrap();
    let echoed = client.expect_close();
    assert_eq!(
        echoed,
        Some((1000, "bye".to_owned())),
        "the server echoed the Close frame"
    );
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    assert_eq!(stat(&f.rt, "live_handles"), 0);
    let _again = f.client();
}

// ----- shutdown ------------------------------------------------------------------------------

#[test]
fn shutdown_closes_the_attached_client_with_going_away_and_frees_the_port() {
    let f = start();
    let mut client = f.client();
    client.new_counter(0);
    let addr = f.server.addr();

    f.server.shutdown();
    let (code, _) = client.expect_close().expect("a Close frame");
    assert_eq!(code, close::GOING_AWAY);
    assert!(!f.bridge.is_connected());
    assert_eq!(stat(&f.rt, "live_handles"), 0);
    assert!(TcpStream::connect(addr).is_err(), "the listener is gone");
    assert!(
        !f.rt.is_shut_down(),
        "shutting the server down leaves the runtime alone"
    );
    // And the address can be bound again.
    std::net::TcpListener::bind(addr).expect("the port is free");
}

#[test]
fn shutdown_is_idempotent_and_dropping_the_server_shuts_it_down() {
    let f = start();
    f.server.shutdown();
    f.server.shutdown();

    let server = Server::start("127.0.0.1:0", quick(), |host| {
        Runtime::new(RuntimeConfig::default(), host)
    })
    .unwrap();
    let (addr, rt) = (server.addr(), server.runtime().clone());
    drop(server);
    assert!(TcpStream::connect(addr).is_err());
    rt.shutdown();
}

#[test]
fn shutdown_does_not_wait_for_a_client_that_ignores_the_close_frame() {
    let f = start();
    let client = f.client();
    // The client never reads again, so it never answers the Close frame.
    let started = Instant::now();
    f.server.shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "took {:?}",
        started.elapsed()
    );
    drop(client);
}

#[test]
fn shutdown_drops_a_connection_that_never_finished_its_handshake() {
    let f = start();
    let stalled = TcpStream::connect(f.server.addr()).unwrap();
    let mut idle = TestClient::connect_raw(&f.url(), f.schema()); // upgraded, no Hello
    let started = Instant::now();
    f.server.shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "took {:?}",
        started.elapsed()
    );
    let close = idle.expect_close();
    assert!(close.is_some_and(|(code, _)| code == close::GOING_AWAY));
    drop(stalled);
}

// ----- limits --------------------------------------------------------------------------------

#[test]
fn a_client_that_stops_reading_is_dropped_and_never_blocks_the_core() {
    let config = ServerConfig {
        max_queued_bytes: 1 << 20,
        ..quick()
    };
    let f = start_with(config, "dev");
    let mut slow = f.client();
    // Ask for far more reply data than the socket buffers and the queue can hold, then never read.
    let big = Bytes(vec![7_u8; 2 << 20]);
    for id in 1..=24_u32 {
        let call = call_payload(
            CallTarget::Function {
                method_id: ECHO_BYTES,
            },
            id,
            &enc(&big),
        );
        if !slow.try_send(Kind::Call, &call) {
            break; // the server already gave up on us
        }
    }

    f.eventually("the slow client was dropped", |f| !f.bridge.is_connected());
    // The core answers immediately to anyone else.
    let started = Instant::now();
    let reply = f.rt.call_sync(&call_payload(
        CallTarget::Function { method_id: SUM },
        1,
        &[enc(&1_i32), enc(&2_i32)].concat(),
    ));
    assert!(!reply.is_empty());
    assert!(started.elapsed() < Duration::from_secs(2));
    let mut next = f.client();
    let (status, _) = next.call(
        CallTarget::Function { method_id: SUM },
        &[enc(&1_i32), enc(&2_i32)].concat(),
    );
    assert_eq!(status, ReplyStatus::Ok);
}

#[test]
fn connections_beyond_the_limit_are_dropped_on_accept() {
    let config = ServerConfig {
        max_connections: 2,
        ..quick()
    };
    let f = start_with(config, "dev");
    let _a = TestClient::connect_raw(&f.url(), f.schema());
    let _b = TestClient::connect_raw(&f.url(), f.schema());
    let mut third = TcpStream::connect(f.server.addr()).unwrap();
    third
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut buf = [0_u8; 16];
    assert!(
        matches!(third.read(&mut buf), Ok(0) | Err(_)),
        "closed without an upgrade"
    );
}

// ----- starting ------------------------------------------------------------------------------

#[test]
fn start_reports_a_runtime_that_cannot_be_built_and_bind_reports_a_busy_address() {
    let bad = Server::start("127.0.0.1:0", quick(), |host| {
        Runtime::new(
            RuntimeConfig {
                mode: "nope".into(),
                ..RuntimeConfig::default()
            },
            host,
        )
    });
    assert!(
        matches!(bad, Err(ServeError::Runtime(InitError::InvalidMode(_)))),
        "{bad:?}"
    );

    let f = start();
    let created: Arc<Mutex<Option<Arc<Runtime>>>> = Arc::default();
    let slot = created.clone();
    let busy = Server::start(f.server.addr(), quick(), move |host| {
        let rt = Runtime::new(
            RuntimeConfig {
                core_threads: 1,
                ..RuntimeConfig::default()
            },
            host,
        )?;
        *slot.lock().unwrap() = Some(rt.clone());
        Ok(rt)
    });
    assert!(matches!(busy, Err(ServeError::Io(_))), "{busy:?}");
    assert!(
        created.lock().unwrap().as_ref().unwrap().is_shut_down(),
        "the runtime start built is shut down when the bind fails"
    );
}

#[test]
fn url_names_the_bound_port() {
    let f = start();
    assert_eq!(
        f.server.url(),
        format!("ws://127.0.0.1:{}", f.server.addr().port())
    );
    assert_ne!(f.server.addr().port(), 0);
    let _ = Message::Ping(Vec::new());
}

// ----- keepalive -----------------------------------------------------------------------------

/// A Hello envelope as a raw client sends it.
fn hello_bytes(schema: u64) -> Vec<u8> {
    let mut payload = keel::wire::Writer::new();
    keel::wire::payload::Hello {
        keel_version: "t",
        schema_hash: schema,
        platform: "raw",
        mode: "dev",
    }
    .encode(&mut payload);
    let mut envelope = keel::wire::Writer::new();
    keel::wire::Envelope::write(&mut envelope, Kind::Hello, 0, schema, payload.as_slice());
    envelope.into_vec()
}

#[test]
fn a_client_that_goes_silent_is_pinged_and_then_dropped_so_it_cannot_hold_the_slot() {
    let config = ServerConfig {
        ping_interval: Duration::from_millis(100),
        ..quick()
    };
    let f = start_with(config, "dev");
    let mut silent = RawWs::connect(f.server.addr());
    silent.binary(&hello_bytes(f.schema()));
    f.eventually("it is attached", |f| f.bridge.is_connected());

    // It never answers a ping (a half-open connection looks exactly like this).
    let started = Instant::now();
    f.eventually("it was dropped", |f| !f.bridge.is_connected());
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "took {:?}",
        started.elapsed()
    );
    let frames = silent.frames_until_end(Duration::from_secs(2));
    assert!(
        frames.iter().any(|frame| frame.opcode == 9),
        "it was pinged first: {frames:?}"
    );

    // The slot is free for the relaunched app.
    let mut next = f.client();
    assert!(next.new_counter(1) > 0);
}

#[test]
fn a_client_that_answers_pings_is_kept() {
    let config = ServerConfig {
        ping_interval: Duration::from_millis(100),
        ..quick()
    };
    let f = start_with(config, "dev");
    let mut client = f.client();
    let handle = client.new_counter(3);
    // Idle for well over three intervals while reading, which is what answers the pings.
    let until = Instant::now() + Duration::from_millis(900);
    while Instant::now() < until {
        let _ = client.recv_within(Duration::from_millis(50));
    }
    assert!(f.bridge.is_connected());
    let (status, body) = client.method(handle, GET, &[]);
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 3));
}

#[test]
fn a_chatty_client_is_never_pinged() {
    let config = ServerConfig {
        ping_interval: Duration::from_millis(150),
        ..quick()
    };
    let f = start_with(config, "dev");
    let mut ws = RawWs::connect(f.server.addr());
    ws.binary(&hello_bytes(f.schema()));
    // A stream of ordinary messages keeps it alive without ever answering a ping.
    let mut w = keel::wire::Writer::new();
    keel::wire::Envelope::write(&mut w, Kind::TimerFired, 1, f.schema(), &enc(&1_u32));
    for _ in 0..12 {
        ws.binary(w.as_slice());
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(f.bridge.is_connected());
    ws.tcp
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let frames = ws.frames_until_end(Duration::from_millis(200));
    assert!(
        frames.iter().all(|frame| frame.opcode != 9),
        "no ping was needed: {frames:?}"
    );
}
