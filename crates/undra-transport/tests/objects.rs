//! Objects a call returned (ADR-040) belong to the client that made the call: a session that
//! disconnects gives them back, one that resumes keeps them, and a `Release` gives back one.
#![cfg(feature = "server")]

mod common;

use std::time::Duration;

use common::*;
use undra::wire::payload::ReplyStatus;
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
