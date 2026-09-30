//! The platform's view of every port: the exact bytes a host receives when core code calls a
//! port through its generated proxy, and the exact bytes it must reply with.
//!
//! No fakes are installed here. The runtime's recording host plays the platform: it is scripted
//! with hand-written reply bytes (what the Swift, Kotlin and TypeScript adapters produce) and it
//! records `(port_id, method_id, args)` of each call. So this test pins, per method:
//!
//! * the ids the proxy sends (a renamed method would silently change them),
//! * the argument encoding, in parameter order,
//! * that the proxy decodes the platform's reply bytes and maps the reply statuses
//!   (`Ok`, typed `Error`) onto the method's return type.

mod common;

use common::hex;
use keel_ports::{
    AppState, Clock, ClockProxy, Fs, FsError, FsProxy, Http, HttpError, HttpMethod, HttpProxy,
    HttpRequest, HttpResponse, Kv, KvProxy, Log, LogProxy, NetKind, Rng, RngProxy, SecureStore,
    SecureStoreProxy, Timer, TimerProxy, on_connectivity_changed, on_lifecycle_changed,
};
use keel_runtime::testing::{PortCallRecord, TestRuntime, port_reply};
use keel_runtime::{Ctx, PortCallOutcome};
use keel_wire::Bytes;
use keel_wire::payload::PortStatus;
use std::sync::{Arc, Mutex};

/// A runtime with nothing bound: every port is the platform's.
fn platform() -> TestRuntime {
    TestRuntime::new()
}

/// A scripted answer that replies with `status` and `body`.
fn answer(status: PortStatus, body: Vec<u8>) -> impl Fn(&PortCallRecord) -> PortCallOutcome {
    move |call| PortCallOutcome::Sync(port_reply(call.port_call_id, status, &body))
}

/// The `(port_id, method_id, args)` of every call the platform received, oldest first.
fn calls(t: &TestRuntime) -> Vec<(u32, u32, Vec<u8>)> {
    t.host()
        .take_port_calls()
        .into_iter()
        .map(|c| (c.port_id, c.method_id, c.args))
        .collect()
}

fn ok(t: &TestRuntime, port: u32, method: u32, body: &str) {
    t.host().script_port_ok(port, method, hex(body));
}

fn ctx(t: &TestRuntime) -> Ctx {
    t.ctx()
}

// ---- sync ports --------------------------------------------------------------------------------

#[test]
fn clock_sends_no_arguments_and_reads_little_endian_integers() {
    let t = platform();
    // now_ms: i64 1_700_000_000_000; monotonic_ns: u64 42_000_000_000; both little-endian.
    ok(&t, 0xcd99_c48e, 0xccc9_4d90, "0068e5cf8b010000");
    ok(&t, 0xcd99_c48e, 0x2cb2_b4bf, "002465c709000000");
    let clock = ClockProxy::new(ctx(&t));
    assert_eq!(clock.now_ms(), 1_700_000_000_000);
    assert_eq!(clock.monotonic_ns(), 42_000_000_000);
    assert_eq!(
        calls(&t),
        [
            (0xcd99_c48e, 0xccc9_4d90, vec![]),
            (0xcd99_c48e, 0x2cb2_b4bf, vec![])
        ]
    );
}

#[test]
fn rng_sends_a_u32_length_and_reads_bytes() {
    let t = platform();
    ok(&t, 0x2513_5bf5, 0x2832_b8ed, "03000000 aabbcc");
    let bytes = RngProxy::new(ctx(&t)).fill(3);
    assert_eq!(bytes, Bytes(vec![0xaa, 0xbb, 0xcc]));
    assert_eq!(calls(&t), [(0x2513_5bf5, 0x2832_b8ed, hex("03000000"))]);
}

#[test]
fn log_sends_level_target_message_and_ignores_the_reply() {
    let t = platform();
    ok(&t, 0x575f_f24a, 0xd49d_5649, "");
    LogProxy::new(ctx(&t)).log(3, "core".into(), "hi".into());
    assert_eq!(
        calls(&t),
        [(
            0x575f_f24a,
            0xd49d_5649,
            hex("03 04000000 636f7265 02000000 6869")
        )]
    );
}

#[test]
fn timer_sends_id_then_delay_as_u32_and_u64() {
    let t = platform();
    ok(&t, 0x00c2_cdd9, 0x923a_766c, "");
    TimerProxy::new(ctx(&t)).set(7, 1000);
    assert_eq!(
        calls(&t),
        [(0x00c2_cdd9, 0x923a_766c, hex("07000000 e803000000000000"))]
    );
}

// ---- Http --------------------------------------------------------------------------------------

#[test]
fn http_sends_the_request_record_and_decodes_the_response_record() {
    let t = platform();
    t.host().script_port(
        0x1ebe_b908,
        0x6b14_df26,
        answer(
            PortStatus::Ok,
            hex("c800 01000000 01000000 6b 01000000 76 02000000 0102"),
        ),
    );
    let request = HttpRequest {
        method: HttpMethod::Patch,
        url: "u".into(),
        headers: vec![],
        body: Some(Bytes(vec![1, 2])),
        timeout_ms: Some(5000),
    };
    let response = t
        .run_until(HttpProxy::new(ctx(&t)).request(request))
        .unwrap();
    assert_eq!(
        response,
        HttpResponse::new(200, vec![1, 2]).with_header("k", "v")
    );
    assert_eq!(
        calls(&t),
        [(
            0x1ebe_b908,
            0x6b14_df26,
            hex("0400 01000000 75 00000000 01 02000000 0102 01 88130000")
        )]
    );
}

#[test]
fn http_reply_status_error_is_the_typed_http_error() {
    let t = platform();
    for (body, expected) in [
        ("0000 01000000 78", HttpError::Network("x".into())),
        ("0100", HttpError::Timeout),
        ("0200", HttpError::Cancelled),
        ("0300 01000000 75", HttpError::InvalidUrl("u".into())),
    ] {
        t.host().script_port(
            0x1ebe_b908,
            0x6b14_df26,
            answer(PortStatus::Error, hex(body)),
        );
        let error = t
            .run_until(HttpProxy::new(ctx(&t)).request(HttpRequest::get("u")))
            .unwrap_err();
        assert_eq!(error, expected);
    }
}

#[test]
fn http_can_be_answered_later_through_port_reply() {
    let t = platform();
    t.host().script_port_async(0x1ebe_b908, 0x6b14_df26);
    let proxy = HttpProxy::new(ctx(&t));
    let slot: Arc<Mutex<Option<Result<HttpResponse, HttpError>>>> = Arc::default();
    let sink = slot.clone();
    t.ctx().spawn(async move {
        let result = proxy.request(HttpRequest::get("u")).await;
        *sink.lock().unwrap() = Some(result);
    });
    t.run_pending();
    assert!(
        slot.lock().unwrap().is_none(),
        "still waiting for the platform"
    );
    let pending = t.host().port_calls();
    assert_eq!(pending.len(), 1);
    t.runtime().port_reply(&port_reply(
        pending[0].port_call_id,
        PortStatus::Ok,
        &hex("9401 00000000 00000000"),
    ));
    t.run_pending();
    assert_eq!(
        slot.lock().unwrap().take(),
        Some(Ok(HttpResponse::new(404, Vec::new())))
    );
}

// ---- Kv and SecureStore ------------------------------------------------------------------------

/// Kv and SecureStore have the same methods under different ids.
fn key_value_contract(name: &str, port: u32, get: u32, set: u32, delete: u32, list: u32, kv: bool) {
    let t = platform();
    let c = ctx(&t);
    let proxy_get = |key: &str| -> Option<Bytes> {
        if kv {
            t.run_until(KvProxy::new(c.clone()).get(key.into()))
        } else {
            t.run_until(SecureStoreProxy::new(c.clone()).get(key.into()))
        }
    };
    // get: `Option<Bytes>` is a 0/1 tag then the bytes.
    ok(&t, port, get, "01 02000000 0102");
    assert_eq!(proxy_get("k"), Some(Bytes(vec![1, 2])), "{name}.get some");
    ok(&t, port, get, "00");
    assert_eq!(proxy_get("k"), None, "{name}.get none");
    // set: key then value; the reply carries nothing.
    ok(&t, port, set, "");
    if kv {
        t.run_until(KvProxy::new(c.clone()).set("k".into(), Bytes(vec![1, 2])));
    } else {
        t.run_until(SecureStoreProxy::new(c.clone()).set("k".into(), Bytes(vec![1, 2])));
    }
    // delete: key.
    ok(&t, port, delete, "");
    if kv {
        t.run_until(KvProxy::new(c.clone()).delete("k".into()));
    } else {
        t.run_until(SecureStoreProxy::new(c.clone()).delete("k".into()));
    }
    // list: prefix, then `Vec<String>`.
    ok(&t, port, list, "02000000 01000000 61 01000000 62");
    let keys = if kv {
        t.run_until(KvProxy::new(c.clone()).list("p".into()))
    } else {
        t.run_until(SecureStoreProxy::new(c.clone()).list("p".into()))
    };
    assert_eq!(keys, ["a", "b"], "{name}.list");
    assert_eq!(
        calls(&t),
        [
            (port, get, hex("01000000 6b")),
            (port, get, hex("01000000 6b")),
            (port, set, hex("01000000 6b 02000000 0102")),
            (port, delete, hex("01000000 6b")),
            (port, list, hex("01000000 70")),
        ],
        "{name} calls"
    );
}

#[test]
fn kv_contract() {
    key_value_contract(
        "Kv",
        0x5389_110d,
        0xf050_bb1a,
        0x6242_7856,
        0x60a3_86b9,
        0x32f1_d03a,
        true,
    );
}

#[test]
fn secure_store_contract() {
    key_value_contract(
        "SecureStore",
        0xc01f_5bea,
        0x5703_6f6f,
        0xe91e_017b,
        0xd57d_b4e2,
        0xf5ba_b8c9,
        false,
    );
}

// ---- Fs ----------------------------------------------------------------------------------------

#[test]
fn fs_contract() {
    let t = platform();
    let fs = FsProxy::new(ctx(&t));
    const PORT: u32 = 0x4ea3_4cab;
    const READ: u32 = 0x01fd_be44;
    const WRITE: u32 = 0x6b70_d47f;
    const DELETE: u32 = 0xa90a_826b;
    const LIST: u32 = 0x4fba_8678;

    ok(&t, PORT, READ, "03000000 010203");
    assert_eq!(t.run_until(fs.read("f".into())), Ok(Bytes(vec![1, 2, 3])));
    ok(&t, PORT, WRITE, "");
    assert_eq!(t.run_until(fs.write("f".into(), Bytes(vec![9]))), Ok(()));
    ok(&t, PORT, DELETE, "");
    assert_eq!(t.run_until(fs.delete("f".into())), Ok(()));
    ok(&t, PORT, LIST, "01000000 01000000 78");
    assert_eq!(t.run_until(fs.list("d".into())), Ok(vec!["x".to_owned()]));
    assert_eq!(
        calls(&t),
        [
            (PORT, READ, hex("01000000 66")),
            (PORT, WRITE, hex("01000000 66 01000000 09")),
            (PORT, DELETE, hex("01000000 66")),
            (PORT, LIST, hex("01000000 64")),
        ]
    );
}

#[test]
fn fs_reply_status_error_is_the_typed_fs_error() {
    let t = platform();
    let fs = FsProxy::new(ctx(&t));
    for (body, expected) in [
        ("0000", FsError::NotFound),
        ("0100", FsError::Denied),
        ("0200 01000000 65", FsError::Io("e".into())),
    ] {
        t.host().script_port(
            0x4ea3_4cab,
            0x01fd_be44,
            answer(PortStatus::Error, hex(body)),
        );
        assert_eq!(t.run_until(fs.read("f".into())), Err(expected));
    }
}

// ---- event ports -------------------------------------------------------------------------------

#[test]
fn connectivity_events_decode_online_then_kind() {
    let t = platform();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let _sub = on_connectivity_changed(&ctx(&t), move |online, kind| {
        sink.lock().unwrap().push((online, kind));
    });
    let (port, method) = (0x1fef_f6ff, 0xb4f2_a010);
    t.runtime().event(port, method, &hex("01 0100"));
    t.runtime().event(port, method, &hex("00 0400"));
    // Truncated, trailing and unknown-variant payloads are dropped, not delivered.
    t.runtime().event(port, method, &hex("01"));
    t.runtime().event(port, method, &hex("01 0100 00"));
    t.runtime().event(port, method, &hex("01 0500"));
    assert_eq!(
        *seen.lock().unwrap(),
        [(true, NetKind::Cellular), (false, NetKind::None)]
    );
}

#[test]
fn lifecycle_events_decode_the_state() {
    let t = platform();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let _sub = on_lifecycle_changed(&ctx(&t), move |state| sink.lock().unwrap().push(state));
    let (port, method) = (0x81c0_afd4, 0x0bc8_2569);
    for payload in ["0000", "0100", "0200", "0300", ""] {
        t.runtime().event(port, method, &hex(payload));
    }
    assert_eq!(
        *seen.lock().unwrap(),
        [AppState::Active, AppState::Inactive, AppState::Background]
    );
}

// ---- unavailable ports -------------------------------------------------------------------------

#[test]
fn an_unscripted_port_is_unavailable_and_the_proxy_says_which() {
    let t = platform();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ClockProxy::new(ctx(&t)).now_ms()
    }));
    let message = match result {
        Ok(_) => panic!("an unavailable sync port must not return a value"),
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default(),
    };
    assert!(
        message.contains("keel: the `Clock` port has no adapter registered (method `now_ms`)"),
        "{message}"
    );
    assert!(message.contains("errors/E0062"), "{message}");
}
