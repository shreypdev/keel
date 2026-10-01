//! Session resume (ADR-051): what the server keeps for a client that dropped, and what a client
//! that comes back finds.
#![cfg(feature = "server")]

mod common;

use std::time::Duration;

use common::*;
use undra::wire::Kind;
use undra::wire::payload::ReplyStatus;
use undra_transport::{ServerConfig, close};

/// A server that keeps a dropped client's objects for `grace`.
fn resuming(grace: Duration) -> Fixture {
    start_with(
        ServerConfig {
            resume_grace: grace,
            ..quick()
        },
        "dev",
    )
}

fn minutes(n: u64) -> Duration {
    Duration::from_secs(60 * n)
}

#[test]
fn a_client_that_comes_back_with_its_token_finds_its_objects_and_its_state() {
    let f = resuming(minutes(10));
    let mut first = f.session_client("tok-first", false);
    let handle = first.new_counter(5);
    first.method(handle, ADD, &enc(&2_i32)); // 7
    first.observe(handle, u32::MAX, true);
    first.recv_kind(Kind::ChangeSet);
    drop(first);

    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    assert_eq!(
        stat(&f.rt, "live_handles"),
        1,
        "the object was kept, not released"
    );
    f.eventually("the disconnect says so", |f| {
        f.log_lines()
            .iter()
            .any(|l| l.contains("client disconnected") && l.contains("1 object(s) kept"))
    });

    let mut back = f.session_client("tok-first", true);
    let (status, body) = back.method(handle, GET, &[]);
    assert_eq!(
        (status, dec::<i32>(&body)),
        (ReplyStatus::Ok, 7),
        "same object, same state"
    );
    // Observing again makes the core send the current values: the client's mirror resyncs.
    back.observe(handle, u32::MAX, true);
    let change_set = back.recv_kind(Kind::ChangeSet);
    assert!(!change_set.payload.is_empty());
    assert!(
        f.log_lines()
            .iter()
            .any(|l| l.contains("client reconnected")
                && l.contains("platform=test")
                && l.contains("1 object(s) kept")),
        "{:?}",
        f.log_lines()
    );
}

#[test]
fn the_objects_of_a_resumed_session_are_the_new_connections_to_give_back() {
    let f = resuming(minutes(10));
    let mut first = f.session_client("tok", false);
    let handle = first.new_counter(1);
    drop(first);
    f.eventually("the slot is free", |f| !f.bridge.is_connected());

    // Back, and gone again: kept again, and resumable again.
    let second = f.session_client("tok", true);
    drop(second);
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    assert_eq!(stat(&f.rt, "live_handles"), 1);
    let mut third = f.session_client("tok", true);
    let (status, body) = third.method(handle, GET, &[]);
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 1));

    // A release by the resumed client releases for real.
    third.release(handle);
    third.new_counter(0); // a round trip: the release was processed
    assert_eq!(
        stat(&f.rt, "live_handles"),
        1,
        "the released one is gone, the new one is there"
    );
}

#[test]
fn what_the_client_released_before_it_dropped_is_not_kept() {
    let f = resuming(minutes(10));
    let mut first = f.session_client("tok", false);
    let kept = first.new_counter(1);
    let released = first.new_counter(2);
    first.release(released);
    first.new_counter(3); // a round trip
    drop(first);
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    assert_eq!(
        stat(&f.rt, "live_handles"),
        2,
        "two kept (the first and the third)"
    );

    let mut back = f.session_client("tok", true);
    assert_eq!(back.method(kept, GET, &[]).0, ReplyStatus::Ok);
    assert_eq!(back.method(released, GET, &[]).0, ReplyStatus::BadRequest);
}

#[test]
fn asking_to_resume_a_session_the_core_does_not_hold_is_session_lost() {
    // A rebuild restarted the core: nothing is retained, and the client is told so by a close
    // code it can act on, after the Hello it needs to check the schema.
    let f = resuming(minutes(10));
    let mut client =
        TestClient::connect_raw(&session_url(&f.url(), "never-seen", true), f.schema());
    client.send_hello(f.schema(), "android", "dev");
    client.expect_frame(Kind::Hello);
    let (code, reason) = client.expect_close().expect("a Close frame");
    assert_eq!(code, close::SESSION_LOST);
    assert!(reason.contains("session lost"), "{reason}");
    f.eventually("the refusal is logged", |f| {
        f.log_lines()
            .iter()
            .any(|l| l.contains("asked to resume session never-se") && l.contains("(android)"))
    });
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    // Not a ban: the same client may connect as a new one.
    let mut fresh = f.session_client("never-seen", false);
    assert_ne!(fresh.new_counter(0), 0);
}

#[test]
fn a_schema_mismatch_is_reported_before_a_lost_session() {
    let f = resuming(minutes(10));
    let mut client = TestClient::connect_raw(&session_url(&f.url(), "tok", true), f.schema());
    client.send_hello(f.schema() ^ 0xff, "ios", "dev");
    client.expect_frame(Kind::Hello);
    let (code, _) = client.expect_close().expect("a Close frame");
    assert_eq!(
        code,
        close::POLICY_VIOLATION,
        "the mismatch wins: the client reports it, once"
    );
}

#[test]
fn a_new_client_releases_what_the_previous_one_left_behind() {
    let f = resuming(minutes(10));
    let mut old = f.session_client("old-token", false);
    old.new_counter(1);
    old.new_counter(2);
    drop(old);
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    assert_eq!(stat(&f.rt, "live_handles"), 2);

    // A relaunched app: another token, and it does not ask to resume anything.
    let mut relaunched = f.session_client("new-token", false);
    f.eventually("the old objects are released", |f| {
        stat(&f.rt, "live_handles") == 0
    });
    relaunched.new_counter(9);
    assert_eq!(stat(&f.rt, "live_handles"), 1);
    f.eventually("it says so", |f| {
        f.log_lines()
            .iter()
            .any(|l| l.contains("released the 2 object(s) of the previous client"))
    });
    drop(relaunched);

    // The old client cannot come back to what is gone.
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    let mut late = TestClient::connect_raw(&session_url(&f.url(), "old-token", true), f.schema());
    late.send_hello(f.schema(), "test", "dev");
    late.expect_frame(Kind::Hello);
    assert_eq!(late.expect_close().unwrap().0, close::SESSION_LOST);
}

#[test]
fn a_session_that_is_not_resumed_in_time_is_released() {
    let f = resuming(Duration::from_millis(300));
    let mut client = f.session_client("tok", false);
    client.new_counter(1);
    drop(client);
    // (Whether the object is still held before the grace passes is covered with a long grace above; with
    // a short one the assertion would race the clock.)
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    f.eventually("the grace passes and the object goes", |f| {
        stat(&f.rt, "live_handles") == 0
    });
    f.eventually("the expiry is logged", |f| {
        f.log_lines()
            .iter()
            .any(|l| l.contains("expired") && l.contains("released 1 object(s)"))
    });
    let mut late = TestClient::connect_raw(&session_url(&f.url(), "tok", true), f.schema());
    late.send_hello(f.schema(), "test", "dev");
    late.expect_frame(Kind::Hello);
    assert_eq!(late.expect_close().unwrap().0, close::SESSION_LOST);
}

#[test]
fn a_client_without_a_token_is_served_as_before() {
    let f = resuming(minutes(10));
    let mut client = f.client();
    client.new_counter(1);
    drop(client);
    f.eventually("its object is released at once", |f| {
        stat(&f.rt, "live_handles") == 0
    });
}

#[test]
fn nothing_is_kept_when_resuming_is_off() {
    let f = start(); // resume_grace is zero
    let mut client = f.session_client("tok", false);
    client.new_counter(1);
    drop(client);
    f.eventually("its object is released at once", |f| {
        stat(&f.rt, "live_handles") == 0
    });
    let mut back = TestClient::connect_raw(&session_url(&f.url(), "tok", true), f.schema());
    back.send_hello(f.schema(), "test", "dev");
    back.expect_frame(Kind::Hello);
    assert_eq!(back.expect_close().unwrap().0, close::SESSION_LOST);
}

#[test]
fn a_client_back_on_a_new_socket_replaces_its_own_stale_one() {
    // A phone that changed network: the old socket is half-open, the server has not noticed.
    // Without the takeover the reconnect would be refused for as long as the keepalive takes.
    let f = start_with(
        ServerConfig {
            resume_grace: minutes(10),
            busy_grace: Duration::from_secs(3),
            ping_interval: Duration::ZERO,
            ..quick()
        },
        "dev",
    );
    let mut stale = f.session_client("tok", false);
    let handle = stale.new_counter(4);

    let mut back = f.session_client("tok", true);
    let (status, body) = back.method(handle, GET, &[]);
    assert_eq!(
        (status, dec::<i32>(&body)),
        (ReplyStatus::Ok, 4),
        "it got its object back"
    );
    assert!(
        f.log_lines()
            .iter()
            .any(|l| l.contains("replacing its previous connection")),
        "{:?}",
        f.log_lines()
    );
    // The stale socket was cut.
    assert!(stale.try_send(Kind::Call, &[]) || stale.expect_close().is_none());
}

#[test]
fn another_session_does_not_take_the_slot_over() {
    let f = resuming(minutes(10));
    let mut first = f.session_client("one", false);
    let handle = first.new_counter(1);
    let mut second = TestClient::connect_raw(&session_url(&f.url(), "two", false), f.schema());
    second.send_hello(f.schema(), "android", "dev");
    second.expect_frame(Kind::Hello);
    assert_eq!(second.expect_close().unwrap().0, close::TRY_AGAIN_LATER);
    assert_eq!(
        first.method(handle, GET, &[]).0,
        ReplyStatus::Ok,
        "the first is untouched"
    );
}

#[test]
fn shutting_the_server_down_releases_what_was_kept() {
    let f = resuming(minutes(10));
    let mut client = f.session_client("tok", false);
    client.new_counter(1);
    drop(client);
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    assert_eq!(stat(&f.rt, "live_handles"), 1);
    f.server.shutdown();
    assert_eq!(stat(&f.rt, "live_handles"), 0);
}

#[test]
fn a_malformed_token_is_an_ordinary_client() {
    let f = resuming(minutes(10));
    let mut client = TestClient::connect_raw(
        &format!("{}/?undra_session=has%20space&undra_resume=1", f.url()),
        f.schema(),
    );
    client.handshake("test", "dev");
    let handle = client.new_counter(1);
    drop(client);
    f.eventually("released at once: no valid token, nothing to resume", |f| {
        stat(&f.rt, "live_handles") == 0
    });
    let _ = handle;
}

#[test]
fn client_churn_never_wedges_the_server_or_leaks_objects() {
    // Reconnects, relaunches and abrupt drops in a row: the server keeps at most the last client's
    // objects, always lets the next client in, and gives everything back at the end.
    let f = resuming(minutes(10));
    for round in 0..25 {
        let token = format!("launch-{round}");
        let mut app = f.session_client(&token, false);
        let kept = app.new_counter(round);
        app.new_counter(round + 1);
        drop(app);
        for _ in 0..3 {
            // The network flaps: the app is back under its own token and gone again.
            let mut back = f.session_client(&token, true);
            let (status, body) = back.method(kept, GET, &[]);
            assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, round));
            drop(back);
        }
        assert!(
            stat(&f.rt, "live_handles") <= 2,
            "round {round}: at most one launch's objects are held"
        );
    }
    f.eventually("the last launch's two objects are all that is held", |f| {
        stat(&f.rt, "live_handles") == 2
    });
    f.server.shutdown();
    assert_eq!(
        stat(&f.rt, "live_handles"),
        0,
        "shutting down gives everything back"
    );
}
