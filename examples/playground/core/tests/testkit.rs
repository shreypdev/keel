//! The playground core under `undra::testing`: a recorded session of the Todos store (the fixture the
//! platform kits replay in previews, `testkit/fixtures/session-todos.json`) and the record/replay
//! round trip of the port traffic of the remote lists.
//!
//! `UNDRA_BLESS=1 cargo test -p playground-core --test testkit` rewrites the fixture.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use playground_core::{RemoteError, RemoteTodo, Todo, Todos};
use undra::meta::ids;
use undra::ports::{HttpMethod, HttpRequest, HttpResponse};
use undra::runtime::testing::{PortCallRecord, RecordingHost, TestRuntime, sync_ok};
use undra::runtime::{PortCallOutcome, RuntimeConfig};
use undra::testing::{EventKind, Recorder, Recording, ReplayError, Replayer, Target};
use undra::wire::payload::{CallTarget, ReplyStatus};
use undra::wire::{Bytes, Decode, Encode, Handle, Reader, Writer};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testkit/fixtures")
        .join(name)
}

/// Compares `recording` with the checked-in fixture, or rewrites it under `UNDRA_BLESS`.
fn check_fixture(name: &str, recording: &Recording) {
    let text = recording.to_json();
    if std::env::var_os("UNDRA_BLESS").is_some() {
        std::fs::write(fixture(name), &text).expect("write the fixture");
        return;
    }
    let on_disk = std::fs::read_to_string(fixture(name)).unwrap_or_else(|_| {
        panic!("{name} is missing: UNDRA_BLESS=1 cargo test -p playground-core --test testkit")
    });
    assert!(
        on_disk == text,
        "{name} is stale: UNDRA_BLESS=1 cargo test -p playground-core --test testkit"
    );
}

fn args(f: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    f(&mut w);
    w.into_vec()
}

fn config() -> RuntimeConfig {
    RuntimeConfig {
        platform: "test".to_owned(),
        mode: "inproc".to_owned(),
        core_threads: 0,
        blocking_threads: 0,
        log_level: 5,
    }
}

// ---- the Todos session ---------------------------------------------------------------------

#[test]
fn a_recorded_todos_session_is_the_checked_in_fixture() {
    // Time is a counter: the recording is the same bytes every run.
    let now = Arc::new(AtomicU64::new(0));
    let clock = now.clone();
    let rec = Arc::new(Recorder::with_clock(
        undra::meta::collect_schema("playground-core").hash(),
        "test",
        move || clock.load(Ordering::SeqCst),
    ));
    let t = TestRuntime::with_host(config(), |h| rec.host(h));
    let mut call_id = 0_u32;
    let mut run = |target: CallTarget, a: Vec<u8>| -> (ReplyStatus, Vec<u8>) {
        call_id += 1;
        now.fetch_add(100, Ordering::SeqCst);
        rec.call(&t, target, call_id, &a);
        t.run_pending();
        let reply = t.take_replies().pop().expect("the call was answered");
        (reply.status, reply.body)
    };
    let (_, body) = run(
        CallTarget::Constructor {
            type_id: ids::type_id("Todos"),
            method_id: ids::method_id("Todos", "new"),
        },
        vec![],
    );
    let handle = u64::from_le_bytes(body[..8].try_into().expect("a handle"));
    rec.observe(&t, handle, u32::MAX, true);
    t.run_pending();
    let on = |name: &str| CallTarget::Method {
        handle: Handle(handle),
        method_id: ids::method_id("Todos", name),
    };
    let mut first = None;
    for title in ["Buy milk", "Walk the dog", "Write the docs"] {
        let (status, body) = run(on("add"), args(|w| w.write_str(title)));
        assert_eq!(status, ReplyStatus::Ok);
        let todo = Todo::decode_exact(&body).expect("the added item");
        first.get_or_insert(todo);
    }
    let first = first.expect("an item was added");
    run(on("toggle"), first.id.encode_to_vec());
    rec.push(EventKind::Release { handle });
    t.runtime().release(handle);

    let recording = rec.finish();
    assert!(recording.events.iter().any(|e| matches!(
        e.kind,
        EventKind::Call {
            target: Target::Method { .. },
            ..
        }
    )));
    check_fixture("session-todos.json", &recording);
    // The fixture is a valid recording that reads back to itself.
    assert_eq!(
        Recording::from_json(&recording.to_json()).unwrap(),
        recording
    );
    let _ = std::any::type_name::<Todos>();
}

// ---- the remote lists' port traffic -----------------------------------------------------------

/// A stand-in for a platform: answers the ports a core uses the way the real adapters do.
fn platform(host: &RecordingHost) {
    use undra::meta::ids::{port_id, port_method_id};
    let kv: Arc<Mutex<BTreeMap<String, Vec<u8>>>> = Arc::default();
    let ticks = Arc::new(AtomicU64::new(0));
    host.script_port(port_id("Clock"), port_method_id("Clock", "now_ms"), |c| {
        sync_ok(c, &1_700_000_000_000_i64.to_le_bytes())
    });
    host.script_port(
        port_id("Clock"),
        port_method_id("Clock", "monotonic_ns"),
        move |c| {
            sync_ok(
                c,
                &(ticks.fetch_add(1_000_000, Ordering::SeqCst)).to_le_bytes(),
            )
        },
    );
    host.script_port(port_id("Rng"), port_method_id("Rng", "fill"), |c| {
        let n = u32::decode_exact(&c.args).unwrap_or(0) as usize;
        sync_ok(c, &Bytes((0..n).map(|i| i as u8).collect()).encode_to_vec())
    });
    host.script_port(port_id("Log"), port_method_id("Log", "log"), |c| {
        sync_ok(c, &[])
    });
    host.script_port(
        port_id("Http"),
        port_method_id("Http", "request"),
        |c: &PortCallRecord| {
            let request = HttpRequest::decode_exact(&c.args).expect("a request");
            let body = match (request.method, request.url.as_str()) {
                (HttpMethod::Post, _) => br#"{"id":7,"title":"Walk","done":false}"#.to_vec(),
                _ => br#"[{"id":7,"title":"Walk","done":false}]"#.to_vec(),
            };
            sync_ok(c, &HttpResponse::new(200, body).encode_to_vec())
        },
    );
    let store = kv.clone();
    host.script_port(port_id("Kv"), port_method_id("Kv", "get"), move |c| {
        let key = String::decode_exact(&c.args).unwrap_or_default();
        let value = store.lock().unwrap().get(&key).cloned().map(Bytes);
        sync_ok(c, &value.encode_to_vec())
    });
    let store = kv.clone();
    host.script_port(port_id("Kv"), port_method_id("Kv", "set"), move |c| {
        let mut r = Reader::new(&c.args);
        if let (Ok(key), Ok(value)) = (String::decode(&mut r), Bytes::decode(&mut r)) {
            store.lock().unwrap().insert(key, value.0);
        }
        sync_ok(c, &[])
    });
    host.script_port(port_id("Kv"), port_method_id("Kv", "delete"), |c| {
        sync_ok(c, &[])
    });
    let store = kv;
    host.script_port(port_id("Kv"), port_method_id("Kv", "list"), move |c| {
        let prefix = String::decode_exact(&c.args).unwrap_or_default();
        let keys: Vec<String> = store
            .lock()
            .unwrap()
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        sync_ok(c, &keys.encode_to_vec())
    });
}

/// Configures the remote and creates one item; returns what the core answered.
fn create_one(
    t: &TestRuntime,
    rec: Option<&Recorder>,
    title: &str,
) -> Result<RemoteTodo, RemoteError> {
    let make = |t: &TestRuntime, id: u32, target: CallTarget, a: &[u8]| match rec {
        Some(rec) => {
            rec.call(t, target, id, a);
        }
        None => {
            t.call(target, id, a);
        }
    };
    make(
        t,
        1,
        CallTarget::Function {
            method_id: ids::function_id("configure_remote"),
        },
        &args(|w| w.write_str("https://api.test")),
    );
    t.run_pending();
    make(
        t,
        2,
        CallTarget::Function {
            method_id: ids::function_id("create_remote_todo"),
        },
        &args(|w| {
            w.write_str("inbox");
            w.write_str(title);
        }),
    );
    t.run_pending();
    let replies = t.take_replies();
    let reply = replies
        .iter()
        .find(|r| r.call_id == 2)
        .expect("create was answered");
    match reply.status {
        ReplyStatus::Ok => Ok(RemoteTodo::decode_exact(&reply.body).expect("an item")),
        ReplyStatus::Error => Err(RemoteError::decode_exact(&reply.body).expect("an error")),
        other => panic!("unexpected status {other:?}"),
    }
}

fn record_a_session() -> (Recording, Result<RemoteTodo, RemoteError>) {
    let now = Arc::new(AtomicU64::new(0));
    let clock = now.clone();
    let rec = Arc::new(Recorder::with_clock(
        undra::meta::collect_schema("playground-core").hash(),
        "test",
        move || clock.fetch_add(3, Ordering::SeqCst),
    ));
    let t = TestRuntime::with_host(config(), |h| rec.host(h));
    platform(t.host());
    let result = create_one(&t, Some(&rec), "Walk");
    (rec.finish(), result)
}

#[test]
fn a_session_recorded_on_one_runtime_replays_on_another() {
    let (recording, recorded_result) = record_a_session();
    let port_calls = recording
        .events
        .iter()
        .filter(|e| matches!(e.kind, EventKind::PortCall { .. }))
        .count();
    assert!(port_calls >= 2, "the core made port calls: {port_calls}");
    assert_eq!(
        recorded_result.as_ref().map(|t| t.title.as_str()),
        Ok("Walk")
    );
    check_fixture("ports-remote-todos.json", &recording);

    // Through JSON and back, as a file would carry it.
    let recording = Recording::from_json(&recording.to_json()).expect("reads back");

    let t = TestRuntime::with_config(config());
    let replayer = Arc::new(Replayer::new(&recording));
    replayer.install(t.host());
    let replayed = create_one(&t, None, "Walk");
    assert_eq!(
        replayed, recorded_result,
        "the core behaves the same on recorded ports"
    );
    assert_eq!(replayer.finish(), Ok(()));
}

#[test]
fn a_replay_that_deviates_reports_typed_errors() {
    let (recording, _) = record_a_session();
    let t = TestRuntime::with_config(config());
    let replayer = Arc::new(Replayer::new(&recording));
    replayer.install(t.host());
    // Another title: the POST body differs from the recorded request.
    let _ = create_one(&t, None, "Something else entirely");
    let errors = replayer.finish().expect_err("the replay deviated");
    assert!(
        errors.iter().any(|e| matches!(e, ReplayError::Mismatch { called, args_differ: true, .. } if called == "Http.request")),
        "{errors:?}"
    );
}

#[test]
fn an_unrecorded_replay_runs_out_and_says_so() {
    let (recording, _) = record_a_session();
    let t = TestRuntime::with_config(config());
    let replayer = Arc::new(Replayer::new(&recording));
    replayer.install(t.host());
    // Nothing is called: every recorded port call is left over.
    drop(t);
    let errors = replayer.finish().expect_err("calls were never made");
    assert!(
        errors
            .iter()
            .all(|e| matches!(e, ReplayError::Unconsumed { .. }))
    );
    let _ = PortCallOutcome::Async;
}
