//! Port calls: in `undra dev` the core runs here and the platform's adapters live in the
//! client, so a port call goes core to client (`PortCall`) and the answer comes back
//! (`PortReply`).
#![cfg(feature = "server")]

mod common;

use common::*;
use undra::runtime::PortError;
use undra::wire::payload::{CallTarget, PortCall, PortStatus, ReplyStatus};
use undra::wire::{Handle, Kind, Reader};

fn port_call(frame: &Frame) -> (u32, u32, u32, Vec<u8>) {
    let call = PortCall::decode(&mut Reader::new(&frame.payload)).unwrap();
    (
        call.port_id,
        call.method_id,
        call.port_call_id,
        call.args.to_vec(),
    )
}

fn send_ask(client: &mut TestClient, handle: u64, x: i32) -> u32 {
    let id = client.next_call_id();
    client.send_call(
        CallTarget::Method {
            handle: Handle(handle),
            method_id: ASK,
        },
        id,
        &enc(&x),
    );
    id
}

#[test]
fn a_port_call_goes_to_the_client_and_its_reply_completes_the_call() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let id = send_ask(&mut client, handle, 20);

    let frame = client.recv_kind(Kind::PortCall);
    assert_eq!(frame.schema, f.schema());
    let (port_id, method_id, port_call_id, args) = port_call(&frame);
    assert_eq!((port_id, method_id), (ECHO_PORT, ECHO_METHOD));
    assert_eq!(dec::<i32>(&args), 20);

    client.port_reply(port_call_id, PortStatus::Ok, &enc(&21_i32));
    let (status, body) = client.await_reply(id);
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 21));
}

#[test]
fn concurrent_port_calls_are_told_apart_by_port_call_id() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let first = send_ask(&mut client, handle, 1);
    let second = send_ask(&mut client, handle, 2);
    let a = port_call(&client.recv_kind(Kind::PortCall));

    let b = port_call(&client.recv_kind(Kind::PortCall));
    assert_ne!(a.2, b.2, "distinct port call ids");

    // Answer them in the opposite order to how they were asked.
    for (_, _, id, args) in [b.clone(), a.clone()] {
        let x: i32 = dec(&args);
        client.port_reply(id, PortStatus::Ok, &enc(&(x * 100)));
    }
    let (status, body) = client.await_reply(first);
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 100));
    let (status, body) = client.await_reply(second);
    assert_eq!((status, dec::<i32>(&body)), (ReplyStatus::Ok, 200));
}

#[test]
fn a_platform_that_does_not_implement_the_port_answers_unavailable() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let id = send_ask(&mut client, handle, 1);
    let (_, _, port_call_id, _) = port_call(&client.recv_kind(Kind::PortCall));
    client.port_reply(port_call_id, PortStatus::Unavailable, &[]);
    // `echo` has no error type, so the generated proxy turns an unavailable port into a panic
    // whose message names the port and the method and says how to bind one (E0062).
    let (status, body) = client.await_reply(id);
    assert_eq!(status, ReplyStatus::Panic);
    let message = dec::<(String, String)>(&body).0;
    assert!(
        message.contains("the `Echo` port has no adapter registered (method `echo`)"),
        "{message}"
    );
    assert!(message.contains("errors.html#E0062"), "{message}");
    // The connection is fine.
    let (status, _) = client.method(handle, GET, &[]);
    assert_eq!(status, ReplyStatus::Ok);
}

#[test]
fn with_no_client_a_port_call_is_unavailable() {
    let f = start();
    assert_eq!(
        f.rt.port_call_sync(ECHO_PORT, ECHO_METHOD, &enc(&1_i32)),
        Err(PortError::Unavailable)
    );
    // And after a client has come and gone.
    let mut client = f.client();
    client.new_counter(0);
    drop(client);
    f.eventually("the client is gone", |f| !f.bridge.is_connected());
    assert_eq!(
        f.rt.port_call_sync(ECHO_PORT, ECHO_METHOD, &enc(&1_i32)),
        Err(PortError::Unavailable)
    );
}

#[test]
fn a_port_call_still_pending_when_the_client_disconnects_fails_instead_of_hanging() {
    let _serial = serial();
    LATE.lock().unwrap().clear();
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    // A detached task (no call waits on it) asks the platform; the platform never answers.
    let (status, _) = client.method(handle, ASK_LATER, &enc(&1_i32));
    assert_eq!(status, ReplyStatus::Ok);
    // The detached task runs after the reply; its PortCall follows.
    client.recv_kind(Kind::PortCall);
    assert_eq!(stat(&f.rt, "pending_port_calls"), 1);
    drop(client);

    eventually("the pending port call was failed", || {
        !LATE.lock().unwrap().is_empty()
    });
    let late = LATE.lock().unwrap().clone();
    assert_eq!(late.len(), 1, "{late:?}");
    assert!(late[0].starts_with("err "), "{late:?}");
    assert_eq!(stat(&f.rt, "pending_port_calls"), 0);
    // The core did not hang and a new client is served.
    f.eventually("the slot is free", |f| !f.bridge.is_connected());
    let mut again = f.client();
    let (status, _) = again.call(
        CallTarget::Function { method_id: SUM },
        &[enc(&1_i32), enc(&2_i32)].concat(),
    );
    assert_eq!(status, ReplyStatus::Ok);
}

#[test]
fn calls_awaiting_a_port_are_cancelled_at_disconnect_and_nothing_is_left_pending() {
    let _serial = serial();
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    send_ask(&mut client, handle, 1);
    client.recv_kind(Kind::PortCall);
    assert_eq!(stat(&f.rt, "active_calls"), 1);
    drop(client); // never answers

    f.eventually("the call was cancelled", |f| {
        stat(&f.rt, "active_calls") == 0
    });
    assert_eq!(stat(&f.rt, "cancelled"), 1);
    f.eventually("no port call is pending", |f| {
        stat(&f.rt, "pending_port_calls") == 0
    });
}

#[test]
fn a_reply_to_a_cancelled_port_call_is_discarded() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let id = send_ask(&mut client, handle, 1);
    let (_, _, port_call_id, _) = port_call(&client.recv_kind(Kind::PortCall));
    client.cancel(id);
    let (status, _) = client.await_reply(id);
    assert_eq!(status, ReplyStatus::Cancelled);
    client.port_reply(port_call_id, PortStatus::Ok, &enc(&5_i32)); // too late
    let (status, _) = client.method(handle, GET, &[]);
    assert_eq!(status, ReplyStatus::Ok, "the late reply changed nothing");
}

#[test]
fn a_synchronous_port_cannot_be_served_by_a_remote_client() {
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let (status, body) = client.method(handle, WALL_NOW, &[]);
    assert_eq!(
        status,
        ReplyStatus::Panic,
        "the proxy of a sync port that is unavailable panics"
    );
    assert!(
        dec::<(String, String)>(&body)
            .0
            .contains("the `Wall` port has no adapter registered (method `now`)")
    );

    // The call still went out (the bridge cannot tell a sync port from an async one), so the
    // client saw it; whatever it answers now is discarded and harmless.
    let call = client
        .frames_of(Kind::PortCall)
        .last()
        .map(|frame| port_call(frame))
        .expect("a PortCall");
    assert_eq!(call.0, WALL_PORT);
    client.port_reply(call.2, PortStatus::Ok, &enc(&1_i64));
    let (status, _) = client.method(handle, GET, &[]);
    assert_eq!(status, ReplyStatus::Ok);
}

#[test]
fn even_a_fire_and_forget_sync_port_is_unavailable_over_a_remote_client() {
    // `Log`, `Clock` and `Rng` are sync ports: a dev core binds Rust implementations for them.
    let f = start();
    let mut client = f.client();
    let handle = client.new_counter(0);
    let (status, body) = client.method(handle, BEEP_TWICE, &[]);
    assert_eq!(status, ReplyStatus::Panic);
    assert!(
        dec::<(String, String)>(&body)
            .0
            .contains("the `Beep` port has no adapter registered (method `beep`)")
    );
    let calls: Vec<_> = client
        .frames_of(Kind::PortCall)
        .into_iter()
        .map(port_call)
        .collect();
    assert_eq!(calls.len(), 1, "the first beep already failed the call");
    assert_eq!(calls[0].0, BEEP_PORT);
}
