//! Objects a call returned (ADR-040) belong to the client that made the call: a session that
//! disconnects gives them back, one that resumes keeps them, and a `Release` gives back one.
#![cfg(feature = "server")]

mod common;

use std::time::Duration;

use common::*;
use undra::wire::payload::{PortCall, ReplyStatus};
use undra::wire::{Kind, Reader};
use undra_transport::ServerConfig;

const CHILD: u32 = undra::meta::ids::method_id("Counter", "child");

fn child_of(client: &mut TestClient, counter: u64) -> u64 {
    let (status, body) = client.method(counter, CHILD, &[]);
    assert_eq!(status, ReplyStatus::Ok);
    dec::<u64>(&body)
}

#[test]
fn a_disconnect_gives_back_what_the_clients_calls_returned() {
    let f = start();
    let mut client = f.client();
    let counter = client.new_counter(1);
    let child = child_of(&mut client, counter);
    assert_eq!(
        child_of(&mut client, counter),
        child,
        "one object, one handle"
    );
    assert_eq!(
        stat(&f.rt, "host_refs"),
        3,
        "the counter, and two references to the child"
    );
    assert_eq!(stat(&f.rt, "origin_refs"), 2);

    drop(client);
    f.eventually("everything the client held is given back", |f| {
        stat(&f.rt, "host_refs") == 0 && stat(&f.rt, "origin_refs") == 0
    });
    assert_eq!(stat(&f.rt, "live_handles"), 0);
}

#[test]
fn a_release_gives_one_reference_back_and_the_disconnect_the_rest() {
    let f = start();
    let mut client = f.client();
    let counter = client.new_counter(1);
    let child = child_of(&mut client, counter);
    assert_eq!(child_of(&mut client, counter), child);
    client.release(child);
    f.eventually("one reference went back", |f| {
        stat(&f.rt, "origin_refs") == 1
    });
    assert_eq!(stat(&f.rt, "host_refs"), 2);
    drop(client);
    f.eventually("the other went back with the disconnect", |f| {
        stat(&f.rt, "host_refs") == 0
    });
}

#[test]
fn a_resumed_session_keeps_its_returned_objects_until_it_releases_or_expires() {
    let f = start_with(
        ServerConfig {
            resume_grace: Duration::from_secs(600),
            ..quick()
        },
        "dev",
    );
    let mut first = f.session_client("tok-objects", false);
    let counter = first.new_counter(1);
    let child = child_of(&mut first, counter);
    drop(first);
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    assert_eq!(
        stat(&f.rt, "host_refs"),
        2,
        "the counter and the child were kept"
    );

    let mut back = f.session_client("tok-objects", true);
    let (status, body) = back.method(child, undra::meta::ids::method_id("Child", "tag"), &[]);
    assert_eq!(
        (status, dec::<u32>(&body)),
        (ReplyStatus::Ok, 7),
        "the same child"
    );
    drop(back);
    f.eventually("the slot is free again", |f| !f.bridge.is_connected());

    // Another client attaches: the session it did not resume is gone, and so are its objects.
    let _other = f.client();
    f.eventually("the previous session's objects were given back", |f| {
        stat(&f.rt, "host_refs") == 0
    });
}

fn hub_handle_of(reply: Vec<u8>) -> u64 {
    // A `Reply` payload: call id (4), status (1), then the body: the constructed handle.
    u64::from_le_bytes(reply[5..13].try_into().unwrap())
}

/// Objects-followups O5: a constructor's reference is counted once, by the session that made it
/// (the runtime's origin ledger skips what a constructor issues), so an `Arc<Self>` constructed
/// twice by a client that disconnects gives back exactly those two, whoever else holds the object.
#[test]
fn a_disconnect_gives_back_one_reference_per_constructor_call_and_no_more() {
    let f = start();
    // The embedding process holds one reference to the runtime's hub (a call made in process).
    let reply = f.rt.call_sync(&undra::runtime::testing::call_payload(
        undra::wire::payload::CallTarget::Constructor {
            type_id: HUB,
            method_id: HUB_SHARED,
        },
        1,
        &[],
    ));
    let hub = hub_handle_of(reply);
    assert_eq!(stat(&f.rt, "host_refs"), 1);

    let mut client = f.client();
    assert_eq!(client.new_hub(), hub, "an interned singleton: one handle");
    assert_eq!(client.new_hub(), hub);
    assert_eq!(
        stat(&f.rt, "host_refs"),
        3,
        "the embedder's and the client's two"
    );
    assert_eq!(
        stat(&f.rt, "origin_refs"),
        0,
        "constructor references are the session's, not the origin ledger's"
    );

    drop(client);
    f.eventually("the client's two references went back", |f| {
        stat(&f.rt, "host_refs") == 1
    });
    assert_eq!(
        f.rt.objects().host_refs_of(undra::wire::Handle(hub)),
        Some(1),
        "the embedder's own reference was not released with them"
    );
}

/// Objects-followups O5: one `Release` gives back one reference, of whichever kind. A hub the
/// client constructed and also had returned by a call is two references; releasing once must leave
/// the other tracked, to be given back at the disconnect (it used to wipe both kinds of tracking).
#[test]
fn a_release_gives_back_one_reference_of_an_object_constructed_and_returned() {
    let f = start();
    let mut client = f.client();
    let counter = client.new_counter(1);
    let hub = client.new_hub();
    let (status, body) = client.method(counter, COUNTER_HUB, &[]);
    assert_eq!(status, ReplyStatus::Ok);
    assert_eq!(dec::<u64>(&body), hub, "the same object");
    assert_eq!(
        stat(&f.rt, "host_refs"),
        3,
        "the counter, and two references to the hub"
    );
    assert_eq!(
        stat(&f.rt, "origin_refs"),
        1,
        "the returned one; the constructed one is the session's"
    );

    client.release(hub);
    f.eventually("one reference went back", |f| stat(&f.rt, "host_refs") == 2);
    drop(client);
    f.eventually("the other one went back with the disconnect", |f| {
        stat(&f.rt, "host_refs") == 0 && stat(&f.rt, "origin_refs") == 0
    });
}

fn port_calls_of(client: &TestClient) -> Vec<(u32, u64)> {
    client
        .frames_of(Kind::PortCall)
        .into_iter()
        .filter_map(|frame| {
            let call = PortCall::decode(&mut Reader::new(&frame.payload)).ok()?;
            (call.port_id == LISTENER_PORT).then(|| {
                let instance = u64::from_le_bytes(call.args[..8].try_into().unwrap());
                (call.method_id, instance)
            })
        })
        .collect()
}

/// Objects-followups O5: every client numbers its callback instances from 1. A session that left
/// keeps no say over the next one's instance 1: the core interns proxies per client, and the left
/// session's proxy (still held by the hub) is never called, nor does it release, towards another
/// session. It used to be interned for the new client's instance 1, whose duplicate reference was
/// then given back to that client at once, as a release of its own listener.
#[test]
fn a_session_that_left_has_no_say_over_the_next_sessions_instances() {
    let _serial = serial();
    let f = start();
    let release = undra::meta::ids::callback_release_id("Listener");

    let mut first = f.client();
    let hub = first.new_hub();
    let (status, _) = first.method(hub, HUB_LISTEN, &enc(&1_u64));
    assert_eq!(status, ReplyStatus::Ok);
    drop(first);
    f.eventually("the first client left", |f| !f.bridge.is_connected());

    let mut second = f.client();
    // The same hub object (its listeners outlived the first client), under a handle of its own: the
    // first client's last reference was its entry's last.
    let hub_second = second.new_hub();
    let (status, _) = second.method(hub_second, HUB_LISTEN, &enc(&1_u64));
    assert_eq!(status, ReplyStatus::Ok);
    let (status, _) = second.method(hub_second, HUB_TELL, &enc(&"hello".to_owned()));
    assert_eq!(status, ReplyStatus::Ok);
    // Only the second client's own proxy speaks, once; nothing is released: its instance 1 was not
    // taken for the first client's (whose proxy is silent now).
    std::thread::sleep(Duration::from_millis(300));
    while let Received::Frame(_) = second.recv_within(Duration::from_millis(200)) {}
    assert_eq!(
        port_calls_of(&second),
        [(LISTENER_NOTE, 1)],
        "one note for its own instance 1, and no release of it"
    );

    // Dropping the listeners: the second client's own reference goes back to it, once; the first
    // client's proxy drops without telling anyone.
    let (status, _) = second.method(hub_second, HUB_FORGET, &[]);
    assert_eq!(status, ReplyStatus::Ok);
    std::thread::sleep(Duration::from_millis(300));
    while let Received::Frame(_) = second.recv_within(Duration::from_millis(200)) {}
    assert_eq!(
        port_calls_of(&second),
        [(LISTENER_NOTE, 1), (release, 1)],
        "exactly one release: its own"
    );
}
