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
    while let Received::Frame(_) = client.recv_within(Duration::from_millis(300)) {}
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
    assert_eq!(
        stat(&f.rt, "live_handles"),
        1,
        "a later shutdown does not release what was handed over"
    );
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
    assert_eq!(
        (status, dec::<i32>(&body)),
        (ReplyStatus::Ok, 7),
        "same handle, same state"
    );
    back.observe(counter, u32::MAX, true);
    let initial = back.recv_kind(Kind::ChangeSet);
    assert!(
        !initial.payload.is_empty(),
        "the restored values are the first change-set"
    );
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
    f.eventually("the call is open in the core", |f| {
        stat(&f.rt, "active_calls") == 1
    });

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
    f.eventually("the call is open in the core", |f| {
        stat(&f.rt, "active_calls") == 1
    });

    let started = std::time::Instant::now();
    let suspended = f.server.suspend(Duration::from_millis(150));
    assert!(
        started.elapsed() >= Duration::from_millis(150),
        "it waited for the settle"
    );
    assert!(!suspended.settled);
    assert_eq!(suspended.cancelled_calls, 1);
    assert!(
        suspended.session.is_some(),
        "the session is handed over all the same"
    );
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
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "a stream does not hold the swap"
    );
    assert!(suspended.settled);
}

#[test]
fn without_resume_grace_there_is_nothing_to_hand_over_and_the_objects_are_released() {
    let f = start(); // resume_grace is zero
    let mut client = f.session_client("tok-none", false);
    client.new_counter(1);
    let suspended = f.server.suspend(SETTLE);
    assert_eq!(suspended.session, None);
    f.eventually("the objects were released", |f| {
        stat(&f.rt, "live_handles") == 0
    });
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
                    let rt = Runtime::new(
                        RuntimeConfig {
                            log_level: 0,
                            ..RuntimeConfig::default()
                        },
                        host,
                    )?;
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
    assert_eq!(
        stat(second.runtime(), "live_handles"),
        0,
        "released, not leaked"
    );
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
    let mut stranger = TestClient::connect_raw(
        &session_url(&second.url(), "someone-else", true),
        second.schema(),
    );
    stranger.send_hello(second.schema(), "test", "dev");
    stranger.expect_frame(Kind::Hello);
    let (code, _) = stranger.expect_close().expect("a Close frame");
    assert_eq!(code, close::SESSION_LOST);
    assert!(stranger.frames_of(Kind::Log).iter().all(|frame| {
        Log::decode(&mut Reader::new(&frame.payload))
            .map_or(true, |log| log.target != NOTICE_TARGET)
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

// ----- review (2026-10-02): the attacks of the adversarial review ---------------------------------

#[test]
fn the_core_cannot_say_a_dev_notice_only_the_server_can() {
    // `undra::dev` is the dev server's own voice (ADR-053, decision 1a): an app's core that logs under
    // that target (`undra_info!(target: "undra::dev", ..)` reaches `Host::log` exactly like this) must
    // not make a status bar say "Reloaded". The record still reaches the terminal (the log sink).
    let f = start_with(quick(), "dev");
    let mut client = f.session_client("tok-spoof", false);
    client.new_counter(1); // a round trip: the client is attached
    f.rt.log(
        undra::runtime::log::INFO,
        NOTICE_TARGET,
        "Reloaded, state kept",
    );
    f.rt.log(undra::runtime::log::INFO, "app", "an ordinary record");
    assert_eq!(notices_of(&mut client), Vec::<String>::new());
    assert!(
        client.frames_of(Kind::Log).iter().any(|frame| {
            Log::decode(&mut Reader::new(&frame.payload)).is_ok_and(|log| log.target == "app")
        }),
        "other records still reach the client"
    );
    assert!(
        f.log_lines()
            .iter()
            .any(|line| line.contains("undra::dev: Reloaded, state kept")),
        "the terminal still shows it: {:?}",
        f.log_lines()
    );
}

/// A successor holding `session`, whose resume grace is `grace`.
fn successor_with_grace(
    addr: std::net::SocketAddr,
    snapshot: &[u8],
    session: Option<KeptSession>,
    notices: AttachNotices,
    grace: Duration,
) -> Server {
    for _ in 0..50 {
        let snapshot = snapshot.to_vec();
        let started = Server::start(
            addr,
            ServerConfig {
                resume_grace: grace,
                inherited_session: session.clone(),
                attach_notices: notices.clone(),
                ..quick()
            },
            move |host| {
                let rt = Runtime::new(
                    RuntimeConfig {
                        log_level: 0,
                        core_threads: 1,
                        blocking_threads: 1,
                        ..RuntimeConfig::default()
                    },
                    host,
                )?;
                rt.restore(&snapshot).expect("the snapshot restores");
                Ok(rt)
            },
        );
        if let Ok(server) = started {
            return server;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("the successor could not bind {addr}");
}

#[test]
fn a_client_that_comes_back_after_the_notice_window_finds_its_state_and_hears_nothing() {
    let first = resuming();
    let addr = first.server.addr();
    let mut client = first.session_client("tok-late", false);
    let counter = client.new_counter(5);
    client.method(counter, ADD, &enc(&4_i32)); // 9
    let suspended = first.server.suspend(SETTLE);
    let snapshot = first.rt.snapshot();
    drop(client);
    drop(first);
    let second = successor_with_grace(
        addr,
        &snapshot,
        suspended.session,
        AttachNotices {
            resumed: Some("kept".to_owned()),
            window: Duration::from_millis(50),
            ..AttachNotices::default()
        },
        minutes(10),
    );
    std::thread::sleep(Duration::from_millis(150));
    let mut back =
        TestClient::connect_session(&second.url(), snapshot_schema(&second), "tok-late", true);
    let (status, body) = back.method(counter, GET, &[]);
    assert_eq!(
        (status, dec::<i32>(&body)),
        (ReplyStatus::Ok, 9),
        "the state, late or not"
    );
    assert!(
        notices_of(&mut back).is_empty(),
        "no notice after the window"
    );
    drop(back);
    second.shutdown();
    second.runtime().shutdown();
}

#[test]
fn an_inherited_session_whose_grace_passes_is_released_and_its_client_told_session_lost() {
    let first = resuming();
    let addr = first.server.addr();
    let mut client = first.session_client("tok-expire", false);
    client.new_counter(5);
    let suspended = first.server.suspend(SETTLE);
    let snapshot = first.rt.snapshot();
    drop(client);
    drop(first);
    let second = successor_with_grace(
        addr,
        &snapshot,
        suspended.session,
        AttachNotices::default(),
        Duration::from_millis(200),
    );
    assert_eq!(stat(second.runtime(), "live_handles"), 1, "held at first");
    eventually(
        "the grace passes and the reaper releases the restored store",
        || stat(second.runtime(), "live_handles") == 0,
    );
    // ADR-051 from here: the objects are gone, so the client is told to load a new core.
    let mut back = TestClient::connect_raw(
        &session_url(&second.url(), "tok-expire", true),
        snapshot_schema(&second),
    );
    back.send_hello(snapshot_schema(&second), "test", "dev");
    back.expect_frame(Kind::Hello);
    assert_eq!(
        back.expect_close().map(|(code, _)| code),
        Some(close::SESSION_LOST)
    );
    second.shutdown();
    second.runtime().shutdown();
}

fn snapshot_schema(server: &Server) -> u64 {
    server.runtime().schema_hash()
}

#[test]
fn a_call_sent_while_the_server_settles_is_not_run_and_is_counted() {
    // The write must not land in the state (the snapshot is taken after the suspend) and must not
    // vanish silently either: `Suspended` counts it, and `undra dev` says so.
    let f = resuming();
    let mut client = f.session_client("tok-dropped", false);
    let counter = client.new_counter(1);
    let hang = client.next_call_id();
    client.send_call(
        undra::wire::payload::CallTarget::Method {
            handle: undra::wire::Handle(counter),
            method_id: HANG,
        },
        hang,
        &[],
    );
    f.eventually("the hanging call is open in the core", |f| {
        stat(&f.rt, "active_calls") == 1
    });
    let suspending = {
        std::thread::scope(|scope| {
            let suspend = scope.spawn(|| f.server.suspend(Duration::from_millis(1500)));
            // Taps until one is not answered: the first one after the server stopped running calls.
            let mut landed = 0;
            loop {
                std::thread::sleep(Duration::from_millis(20));
                let id = client.next_call_id();
                client.send_call(
                    undra::wire::payload::CallTarget::Method {
                        handle: undra::wire::Handle(counter),
                        method_id: ADD,
                    },
                    id,
                    &enc(&10_i32),
                );
                // A dev client also hears the server's log records; a tap is answered or the
                // connection closes (or nothing comes at all).
                let answered = loop {
                    match client.recv_within(Duration::from_secs(3)) {
                        Received::Frame(frame) if frame.kind == Kind::Reply => break true,
                        Received::Frame(frame) if frame.kind == Kind::Log => {}
                        Received::Frame(other) => panic!("unexpected {other:?}"),
                        Received::Closed(_) | Received::Silence => break false,
                    }
                };
                if !answered {
                    break;
                }
                landed += 1;
            }
            (suspend.join().unwrap(), landed)
        })
    };
    let (suspended, landed) = suspending;
    assert_eq!(suspended.cancelled_calls, 1, "{suspended:?}");
    assert_eq!(suspended.dropped_calls, 1, "{suspended:?}");
    assert!(!suspended.settled);
    let revived = start();
    revived.rt.restore(&f.rt.snapshot()).unwrap();
    let mut other = revived.client();
    let (status, body) = other.method(counter, GET, &[]);
    assert_eq!(
        (status, dec::<i32>(&body)),
        (ReplyStatus::Ok, 1 + 10 * landed),
        "the answered taps are in the state, the one that was not run is not"
    );
    assert!(
        f.log_lines()
            .iter()
            .any(|line| line.contains("1 sent meanwhile were not run")),
        "{:?}",
        f.log_lines()
    );
}

#[test]
fn with_two_clients_only_the_attached_one_has_a_session_to_hand_over() {
    // `undra dev` serves one client at a time (ADR-051): a simulator and an emulator take turns on the
    // slot, and the one that is refused (1013) holds nothing in this core. So the reload hands over
    // exactly one session, the attached client's; the other client loads afresh when it gets the slot.
    let f = resuming();
    let mut first = f.session_client("tok-sim", false);
    let counter = first.new_counter(3);
    let mut second = TestClient::connect_raw(&session_url(&f.url(), "tok-emu", false), f.schema());
    second.send_hello(f.schema(), "test", "dev");
    second.expect_frame(Kind::Hello);
    assert_eq!(
        second.expect_close().map(|(code, _)| code),
        Some(close::TRY_AGAIN_LATER)
    );
    let suspended = f.server.suspend(SETTLE);
    assert_eq!(
        suspended.session,
        Some(KeptSession {
            token: "tok-sim".to_owned(),
            handles: vec![counter],
        })
    );
}

#[test]
fn every_tap_racing_the_suspend_is_answered_or_not_run_and_only_answered_ones_are_in_the_state() {
    // Taps pipelined against the start of the suspend, with no call open (so the settle ends at once:
    // the narrowest window). A call is decided under the connection's lock, the lock the suspend takes
    // to count the open calls after it froze the server, so each tap either runs and is answered
    // before the Close frame, or is not run (counted, or arrives after the close). What must never
    // happen: a tap that ran (its write is in the snapshot) and whose reply was lost.
    for round in 0..30_u64 {
        let f = resuming();
        let mut client = f.session_client(&format!("tok-race-{round}"), false);
        let counter = client.new_counter(0);
        let (answered, sent, suspended) = std::thread::scope(|scope| {
            let suspend = scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(3 + round % 11));
                f.server.suspend(Duration::from_millis(200))
            });
            let (mut answered, mut sent) = (0_i32, 0_u32);
            'taps: loop {
                for _ in 0..4 {
                    let id = client.next_call_id();
                    let call = undra::runtime::testing::call_payload(
                        undra::wire::payload::CallTarget::Method {
                            handle: undra::wire::Handle(counter),
                            method_id: ADD,
                        },
                        id,
                        &enc(&1_i32),
                    );
                    if !client.try_send(Kind::Call, &call) {
                        break 'taps;
                    }
                    sent += 1;
                }
                loop {
                    match client.recv_within(Duration::from_millis(20)) {
                        Received::Frame(frame) if frame.kind == Kind::Reply => {
                            let reply = undra::wire::payload::Reply::decode(&mut Reader::new(
                                &frame.payload,
                            ))
                            .unwrap();
                            assert_eq!(reply.status, ReplyStatus::Ok);
                            answered += 1;
                        }
                        Received::Frame(_) => {}
                        Received::Silence => break,
                        Received::Closed(_) => break 'taps,
                    }
                }
            }
            (answered, sent, suspend.join().unwrap())
        });
        let revived = start();
        revived.rt.restore(&f.rt.snapshot()).unwrap();
        let mut other = revived.client();
        let (status, body) = other.method(counter, GET, &[]);
        assert_eq!(status, ReplyStatus::Ok);
        assert_eq!(
            dec::<i32>(&body),
            answered,
            "round {round}: the state holds exactly the answered taps ({answered} of {sent} sent, {suspended:?})"
        );
        assert!(
            u32::try_from(answered).unwrap() + u32::try_from(suspended.dropped_calls).unwrap()
                <= sent,
            "round {round}: {answered} answered + {suspended:?} > {sent} sent"
        );
        assert_eq!(
            suspended.cancelled_calls, 0,
            "round {round}: no call was open at the deadline"
        );
    }
}

#[test]
fn a_client_that_keeps_sending_and_reads_late_still_finds_the_close_frame() {
    // The raw client of `undra dev`'s tests reads nothing while a rebuild runs and sends calls
    // meanwhile; what the server wrote before it hung up must be the first thing it finds when it
    // reads again, however long it was away and whatever it sent into the closed socket.
    let f = resuming();
    let mut client = f.session_client("tok-sends", false);
    let counter = client.new_counter(0);
    let call = |client: &mut TestClient| {
        let id = client.next_call_id();
        let payload = undra::runtime::testing::call_payload(
            undra::wire::payload::CallTarget::Method {
                handle: undra::wire::Handle(counter),
                method_id: ADD,
            },
            id,
            &enc(&1_i32),
        );
        client.try_send(Kind::Call, &payload)
    };

    let started = std::time::Instant::now();
    let suspended = std::thread::scope(|scope| {
        let suspending = scope.spawn(|| f.server.suspend(SETTLE));
        while !suspending.is_finished() {
            call(&mut client);
            std::thread::sleep(Duration::from_millis(20));
        }
        suspending.join().expect("the suspend finished")
    });
    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "the client never answered the Close frame, so the server waited out `close_timeout` ({:?})",
        started.elapsed()
    );
    assert!(suspended.dropped_calls > 0, "{suspended:?}");
    // Still sending into the socket the server has closed: the answer to each is a reset.
    for _ in 0..10 {
        call(&mut client);
        std::thread::sleep(Duration::from_millis(20));
    }

    let (code, reason) = client
        .expect_close()
        .expect("a Close frame, not a bare reset");
    assert_eq!(code, close::GOING_AWAY);
    assert!(reason.contains("reloading"), "{reason}");
}
