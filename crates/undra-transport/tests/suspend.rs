//! Suspending a server for a reload (ADR-053): the quiesce that lets `undra dev` take a core's
//! state and hand its session to the core that replaces it, and the notices the new server tells
//! the clients that come back.
#![cfg(feature = "server")]

mod common;

use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use undra::runtime::{Runtime, RuntimeConfig};
use undra::wire::payload::{Log, ReplyStatus};
use undra::wire::{Kind, Reader};
use undra_transport::{AttachNotices, KeptSession, NOTICE_TARGET, Server, ServerConfig, close};

fn minutes(n: u64) -> Duration {
    Duration::from_secs(60 * n)
}

fn resuming() -> Fixture {
    start_with(
        ServerConfig {
            resume_grace: minutes(10),
            ..quick()
        },
        "dev",
    )
}

const SETTLE: Duration = Duration::from_secs(2);

/// A second server on `addr` whose runtime was restored from `snapshot` before it listened, as
/// the dev runner does, with the session and the notices it was given.
fn successor(
    addr: std::net::SocketAddr,
    snapshot: &[u8],
    session: Option<KeptSession>,
    notices: AttachNotices,
) -> Fixture {
    let snapshot = snapshot.to_vec();
    let config = ServerConfig {
        resume_grace: minutes(10),
        inherited_session: session,
        attach_notices: notices,
        ..quick()
    };
    // The previous listener was dropped by `suspend`; the OS may take a moment to hand the port over.
    let mut last = None;
    for _ in 0..50 {
        let snapshot = snapshot.clone();
        let config = config.clone();
        match Server::start(addr, config, move |host| {
            let rt = Runtime::new(
                RuntimeConfig {
                    platform: "rust".into(),
                    mode: "dev".into(),
                    core_threads: 1,
                    blocking_threads: 1,
                    log_level: 0,
                },
                host,
            )?;
            rt.restore(&snapshot).expect("the snapshot restores");
            Ok(rt)
        }) {
            Ok(server) => {
                let logs: Logs = Arc::default();
                let sink = logs.clone();
                server.bridge().set_log_sink(move |level, target, message| {
                    sink.lock()
                        .unwrap()
                        .push((level, target.to_owned(), message.to_owned()));
                });
                return Fixture {
                    rt: server.runtime().clone(),
                    bridge: server.bridge().clone(),
                    server,
                    logs,
                };
            }
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    panic!("the successor could not bind {addr}: {last:?}");
}

/// The `undra::dev` sentences a client receives within a short while.
fn notices_of(client: &mut TestClient) -> Vec<String> {
    loop {
        match client.recv_within(Duration::from_millis(300)) {
            Received::Frame(_) => {}
            Received::Closed(_) | Received::Silence => break,
        }
    }
    client
        .seen
        .iter()
        .filter(|frame| frame.kind == Kind::Log)
        .filter_map(|frame| {
            let log = Log::decode(&mut Reader::new(&frame.payload)).ok()?;
            (log.target == NOTICE_TARGET).then(|| log.message.to_owned())
        })
        .collect()
}


#[test]
fn suspend_stops_listening_closes_the_client_and_hands_over_its_session_unreleased() {
    let f = resuming();
    let mut client = f.session_client("tok-reload", false);
    let counter = client.new_counter(5);
    client.method(counter, ADD, &enc(&2_i32));
    client.observe(counter, u32::MAX, true);
    client.recv_kind(Kind::ChangeSet);

    let suspended = f.server.suspend(SETTLE);

    assert!(suspended.settled);
    assert_eq!(suspended.cancelled_calls, 0);
    let session = suspended.session.expect("a session is handed over");
    assert_eq!(session.token, "tok-reload");
    assert_eq!(session.handles, [counter]);
    // Held, not released: the snapshot has the store.
    assert_eq!(stat(&f.rt, "live_handles"), 1);

    let (code, reason) = client.expect_close().expect("a Close frame");
    assert_eq!(code, close::GOING_AWAY);
    assert!(reason.contains("reloading"), "{reason}");

    assert!(
        TcpStream::connect_timeout(&f.server.addr(), Duration::from_millis(500)).is_err(),
        "the address is free for the next core"
    );

    // Everything after is idempotent: nothing more to hand over, nothing to release.
    let again = f.server.suspend(SETTLE);
    assert_eq!(again.session, None);
    f.server.shutdown();
    assert_eq!(stat(&f.rt, "live_handles"), 1, "a later shutdown does not release what was handed over");
}

#[test]
fn the_state_of_the_old_core_and_the_client_session_carry_over_to_the_new_one() {
    let first = resuming();
    let addr = first.server.addr();
    let mut client = first.session_client("tok-carry", false);
    let counter = client.new_counter(5);
    client.method(counter, ADD, &enc(&2_i32)); // 7
    client.observe(counter, u32::MAX, true);
    client.recv_kind(Kind::ChangeSet);

    let suspended = first.server.suspend(SETTLE);
    let snapshot = first.rt.snapshot();
    drop(client);
    drop(first);

    let second = successor(
        addr,
        &snapshot,
        suspended.session,
        AttachNotices {
            resumed: Some("Reloaded, state kept".to_owned()),
            ..AttachNotices::default()
        },
    );
    // The client comes back the way ADR-051 has it: same token, resume.
    let mut back = second.session_client("tok-carry", true);
    let (status, body) = back.method(counter, GET, &[]);
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 7), "same handle, same state");
    back.observe(counter, u32::MAX, true);
    let initial = back.recv_kind(Kind::ChangeSet);
    assert!(!initial.payload.is_empty(), "the restored values are the first change-set");
    assert_eq!(notices_of(&mut back), ["Reloaded, state kept"]);

    // Its objects are its own again: a release releases for real.
    back.release(counter);
    back.new_counter(0); // a round trip
    assert_eq!(stat(&second.rt, "live_handles"), 1);
}

#[test]
fn a_call_that_is_open_when_the_server_is_suspended_finishes_first() {
    let f = resuming();
    let mut client = f.session_client("tok-settle", false);
    let counter = client.new_counter(1);
    // 20 ms in the core, then `count += 5`.
    let id = client.next_call_id();
    client.send_call(
        undra::wire::payload::CallTarget::Method {
            handle: undra::wire::Handle(counter),
            method_id: SLOW_ADD,
        },
        id,
        &enc(&5_i32),
    );
    f.eventually("the call is open in the core", |f| stat(&f.rt, "active_calls") == 1);

    let suspended = f.server.suspend(SETTLE);
    assert!(suspended.settled, "{suspended:?}");

    // Its effect is in the snapshot taken after the suspend, and its reply was delivered.
    let revived = start();
    revived.rt.restore(&f.rt.snapshot()).unwrap();
    let mut other = revived.client();
    let (status, body) = other.method(counter, GET, &[]);
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 6));
    let (status, body) = client.await_reply(id);
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 6));
}

#[test]
fn a_call_that_does_not_finish_within_the_settle_is_cancelled_and_counted() {
    let _serial = serial();
    let f = resuming();
    let mut client = f.session_client("tok-hang", false);
    let counter = client.new_counter(1);
    let id = client.next_call_id();
    client.send_call(
        undra::wire::payload::CallTarget::Method {
            handle: undra::wire::Handle(counter),
            method_id: HANG,
        },
        id,
        &[],
    );
    let dropped_before = DROPPED.load(std::sync::atomic::Ordering::SeqCst);
    f.eventually("the call is open in the core", |f| stat(&f.rt, "active_calls") == 1);

    let started = std::time::Instant::now();
    let suspended = f.server.suspend(Duration::from_millis(150));
    assert!(started.elapsed() >= Duration::from_millis(150), "it waited for the settle");
    assert!(!suspended.settled);
    assert_eq!(suspended.cancelled_calls, 1);
    assert!(suspended.session.is_some(), "the session is handed over all the same");
    f.eventually("the core dropped the future", |_| {
        DROPPED.load(std::sync::atomic::Ordering::SeqCst) > dropped_before
    });
    assert!(client.expect_close().is_some());
}

#[test]
fn a_streaming_call_is_not_waited_for() {
    let f = resuming();
    let mut client = f.session_client("tok-stream", false);
    let counter = client.new_counter(1);
    let id = client.next_call_id();
    client.send_call(
        undra::wire::payload::CallTarget::Method {
            handle: undra::wire::Handle(counter),
            method_id: ENDLESS,
        },
        id,
        &[],
    );
    let (status, _) = client.await_reply(id);
    assert_eq!(status, ReplyStatus::StreamOpened);
    let started = std::time::Instant::now();
    let suspended = f.server.suspend(Duration::from_secs(5));
    assert!(started.elapsed() < Duration::from_secs(4), "a stream does not hold the swap");
    assert!(suspended.settled);
}

#[test]
fn without_resume_grace_there_is_nothing_to_hand_over_and_the_objects_are_released() {
    let f = start(); // resume_grace is zero
    let mut client = f.session_client("tok-none", false);
    client.new_counter(1);
    let suspended = f.server.suspend(SETTLE);
    assert_eq!(suspended.session, None);
    f.eventually("the objects were released", |f| stat(&f.rt, "live_handles") == 0);
}

#[test]
fn a_session_that_cannot_be_held_is_released_and_said() {
    // Resuming is off: the inherited session cannot be held, so its objects must not leak.
    let first = resuming();
    let mut client = first.session_client("tok-inherit", false);
    let counter = client.new_counter(1);
    let suspended = first.server.suspend(SETTLE);
    let snapshot = first.rt.snapshot();
    let addr = first.server.addr();
    drop(client);
    drop(first);

    let snapshot_for_start = snapshot.clone();
    let second = {
        let mut server = None;
        for _ in 0..50 {
            let snapshot = snapshot_for_start.clone();
            let session = suspended.session.clone();
            let started = Server::start(
                addr,
                ServerConfig {
                    inherited_session: session,
                    ..quick() // resume_grace: zero
                },
                move |host| {
                    let rt = Runtime::new(RuntimeConfig { log_level: 0, ..RuntimeConfig::default() }, host)?;
                    rt.restore(&snapshot).unwrap();
                    Ok(rt)
                },
            );
            if let Ok(s) = started {
                server = Some(s);
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        server.expect("bound")
    };
    let _ = counter;
    assert_eq!(stat(second.runtime(), "live_handles"), 0, "released, not leaked");
    second.shutdown();
    second.runtime().shutdown();
}

#[test]
fn every_client_that_attaches_in_the_window_is_told_once() {
    let f = start_with(
        ServerConfig {
            resume_grace: minutes(10),
            attach_notices: AttachNotices {
                resumed: Some("kept".to_owned()),
                fresh: Some("reset".to_owned()),
                window: minutes(1),
            },
            ..quick()
        },
        "dev",
    );
    // A new client is told the `fresh` sentence; so is the next one (another session).
    let mut first = f.session_client("a", false);
    assert_eq!(notices_of(&mut first), ["reset"]);
    drop(first);
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    let mut second = f.session_client("b", false);
    assert_eq!(notices_of(&mut second), ["reset"]);
    drop(second);
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    // The same client again, inside the window, is not told twice.
    let mut again = f.session_client("b", false);
    assert!(notices_of(&mut again).is_empty());
    // A client that is not a dev client gets it too: the notice is not a devtools record.
    drop(again);
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    let mut prod = TestClient::connect_raw(&session_url(&f.url(), "c", false), f.schema());
    prod.handshake("test", "prod");
    assert_eq!(notices_of(&mut prod), ["reset"]);
}

#[test]
fn a_resumed_client_is_told_the_resumed_sentence_and_a_refused_one_nothing() {
    let first = resuming();
    let addr = first.server.addr();
    let mut client = first.session_client("tok-r", false);
    client.new_counter(1);
    let suspended = first.server.suspend(SETTLE);
    let snapshot = first.rt.snapshot();
    drop(client);
    drop(first);
    let second = successor(
        addr,
        &snapshot,
        suspended.session,
        AttachNotices {
            resumed: Some("kept".to_owned()),
            fresh: Some("reset".to_owned()),
            window: minutes(1),
        },
    );
    // Asking to resume a session the core does not hold: 4001, and no notice (it is not attached).
    let mut stranger = TestClient::connect_raw(&session_url(&second.url(), "someone-else", true), second.schema());
    stranger.send_hello(second.schema(), "test", "dev");
    stranger.expect_frame(Kind::Hello);
    let (code, _) = stranger.expect_close().expect("a Close frame");
    assert_eq!(code, close::SESSION_LOST);
    assert!(stranger.frames_of(Kind::Log).iter().all(|frame| {
        Log::decode(&mut Reader::new(&frame.payload)).map_or(true, |log| log.target != NOTICE_TARGET)
    }));
    second.eventually("the slot is free", |f| !f.bridge.is_connected());

    let mut back = second.session_client("tok-r", true);
    assert_eq!(notices_of(&mut back), ["kept"]);
}

#[test]
fn no_notice_is_sent_after_the_window() {
    let f = start_with(
        ServerConfig {
            attach_notices: AttachNotices {
                fresh: Some("reset".to_owned()),
                window: Duration::ZERO,
                ..AttachNotices::default()
            },
            ..quick()
        },
        "dev",
    );
    let mut client = f.client();
    assert!(notices_of(&mut client).is_empty());
}

#[test]
fn a_server_with_no_notices_never_says_one() {
    // The default: `undra dev`'s first start, an in-process-style embedding, every test above this file.
    let f = start();
    let mut client = f.client();
    assert!(notices_of(&mut client).is_empty());
}
