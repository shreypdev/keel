//! Behaviour tests for `#[keel::port]`: proxies, accessors, Rust-side dispatchers, event
//! subscriptions and the async-to-boxed-future rewrite, run against the real runtime.
#![forbid(unsafe_code)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use keel::meta::{PortKind, TypeRef, collect_schema, ids};
use keel::runtime::{Ctx, Port, PortDispatch, PortDispatcher, PortError};
use keel::wire::{Bytes, Decode, Encode, Reader, Writer};
use keel_macros as k;

mod support;
use support::Runtime;
use support::testing::block_on;

#[k::error]
#[derive(Clone, PartialEq)]
pub enum HttpError {
    #[error("network: {0}")]
    Network(String),
    #[error("timeout")]
    Timeout,
}

#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct HttpRequest {
    pub url: String,
    pub retries: u8,
}

#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Bytes,
}

#[k::api]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NetKind {
    Wifi,
    Cellular,
}

/// Talks HTTP.
#[k::port]
pub trait Http {
    /// Sends a request.
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>;
    async fn ping(&self, url: String) -> bool;
    async fn fire(&self, msg: String);
    async fn check(&self, url: String) -> Result<(), HttpError>;
}

#[k::port(sync)]
pub trait Clock {
    fn now_ms(&self) -> i64;
    fn monotonic_ns(&self) -> u64;
}

#[k::port]
pub trait Log {
    fn log(&self, level: u8, target: String, message: String);
}

#[k::port]
pub trait SecureStore {
    fn get(&self, key: String) -> Result<Option<Bytes>, HttpError>;
    fn delete(&self, key: String);
}

#[k::port(event)]
pub trait Connectivity {
    fn changed(&self, online: bool, kind: NetKind);
}

#[k::port(event)]
pub trait Lifecycle {
    fn changed(&self, state: u8);
    fn low_memory(&self);
}

fn id(trait_name: &str, method: &str) -> u32 {
    ids::port_method_id(trait_name, method)
}

fn encode(f: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    f(&mut w);
    w.into_vec()
}

fn message_of(panic: Box<dyn std::any::Any + Send>) -> String {
    match panic.downcast::<String>() {
        Ok(s) => *s,
        Err(other) => other
            .downcast::<&str>()
            .map_or_else(|_| "<non-string panic>".to_owned(), |s| (*s).to_owned()),
    }
}

// ---------------------------------------------------------------------------------------------
// Proxies over a foreign binding
// ---------------------------------------------------------------------------------------------

fn runtime_with_host() -> (Runtime, Ctx, Arc<Mutex<Vec<String>>>) {
    let rt = Runtime::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    rt.bind_foreign(<dyn Http as Port>::PORT_ID, move |method_id, args| {
        if method_id == id("Http", "request") {
            let req = HttpRequest::decode_exact(args).expect("args decode as HttpRequest");
            log.lock().unwrap().push(format!("request {}", req.url));
            return match req.url.as_str() {
                "timeout" => Err(PortError::Failed(HttpError::Timeout.encode_to_vec())),
                "garbage" => Ok(vec![1, 2, 3]),
                "gone" => Err(PortError::Unavailable),
                url => Ok(HttpResponse {
                    status: 200 + u16::from(req.retries),
                    body: Bytes(url.as_bytes().to_vec()),
                }
                .encode_to_vec()),
            };
        }
        if method_id == id("Http", "ping") {
            let url = String::decode_exact(args).unwrap();
            log.lock().unwrap().push(format!("ping {url}"));
            return Ok(true.encode_to_vec());
        }
        if method_id == id("Http", "fire") {
            let msg = String::decode_exact(args).unwrap();
            log.lock().unwrap().push(format!("fire {msg}"));
            return Ok(Vec::new());
        }
        if method_id == id("Http", "check") {
            let url = String::decode_exact(args).unwrap();
            return if url == "ok" {
                Ok(Vec::new())
            } else {
                Err(PortError::Failed(HttpError::Network(url).encode_to_vec()))
            };
        }
        panic!("unexpected method {method_id}");
    });
    let ctx = rt.ctx();
    (rt, ctx, seen)
}

#[test]
fn the_accessor_returns_a_proxy_when_nothing_is_bound_in_rust() {
    let (_rt, ctx, seen) = runtime_with_host();
    let http = http(&ctx);
    let response = block_on(http.request(HttpRequest {
        url: "keel.dev".into(),
        retries: 2,
    }))
    .unwrap();
    assert_eq!(response.status, 202);
    assert_eq!(response.body, Bytes(b"keel.dev".to_vec()));
    assert_eq!(*seen.lock().unwrap(), ["request keel.dev"]);
}

#[test]
fn typed_errors_come_back_as_the_methods_error_type() {
    let (_rt, ctx, _) = runtime_with_host();
    let http = http(&ctx);
    let error = block_on(http.request(HttpRequest {
        url: "timeout".into(),
        retries: 0,
    }))
    .unwrap_err();
    assert_eq!(error, HttpError::Timeout);
    assert_eq!(block_on(http.check("ok".into())), Ok(()));
    assert_eq!(
        block_on(http.check("boom".into())),
        Err(HttpError::Network("boom".into()))
    );
}

#[test]
fn plain_and_unit_returns() {
    let (_rt, ctx, seen) = runtime_with_host();
    let http = http(&ctx);
    assert!(block_on(http.ping("x".into())));
    block_on(http.fire("hello".into()));
    assert_eq!(*seen.lock().unwrap(), ["ping x", "fire hello"]);
}

#[test]
fn an_unavailable_port_panics_with_a_message_naming_port_and_method() {
    let rt = Runtime::new();
    let ctx = rt.ctx();
    let panic = catch_unwind(AssertUnwindSafe(|| {
        block_on(http(&ctx).ping("x".into()));
    }))
    .unwrap_err();
    let message = message_of(panic);
    assert!(
        message.contains("keel: port call `Http.ping` failed"),
        "{message}"
    );
    assert!(message.contains("Unavailable"), "{message}");
}

#[test]
fn a_reply_that_does_not_decode_panics_with_a_clear_message() {
    let (_rt, ctx, _) = runtime_with_host();
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _ = block_on(http(&ctx).request(HttpRequest {
            url: "garbage".into(),
            retries: 0,
        }));
    }))
    .unwrap_err();
    let message = message_of(panic);
    assert!(
        message.contains("port `Http.request` replied with a value that does not decode"),
        "{message}"
    );
    // A transport-level failure on a `Result` method is not a typed error either.
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _ = block_on(http(&ctx).request(HttpRequest {
            url: "gone".into(),
            retries: 0,
        }));
    }))
    .unwrap_err();
    assert!(message_of(panic).contains("Http.request` failed"));
}

#[test]
fn sync_ports_call_through_port_call_sync() {
    let rt = Runtime::new();
    rt.bind_foreign(<dyn Clock as Port>::PORT_ID, |method_id, args| {
        assert!(args.is_empty());
        if method_id == id("Clock", "now_ms") {
            Ok(1_700_000_000_000_i64.encode_to_vec())
        } else if method_id == id("Clock", "monotonic_ns") {
            Ok(42_u64.encode_to_vec())
        } else {
            panic!("unexpected method");
        }
    });
    let clock = clock(&rt.ctx());
    assert_eq!(clock.now_ms(), 1_700_000_000_000);
    assert_eq!(clock.monotonic_ns(), 42);
}

#[test]
fn fire_and_forget_sync_methods_ignore_the_reply() {
    let rt = Runtime::new();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    rt.bind_foreign(<dyn Log as Port>::PORT_ID, move |_, args| {
        let mut r = Reader::new(args);
        let level = r.read_u8().unwrap();
        let target = r.read_str().unwrap().to_owned();
        let message = r.read_str().unwrap().to_owned();
        r.finish().unwrap();
        sink.lock()
            .unwrap()
            .push(format!("{level} {target} {message}"));
        Ok(Vec::new())
    });
    log(&rt.ctx()).log(3, "core".into(), "hi".into());
    assert_eq!(*lines.lock().unwrap(), ["3 core hi"]);
}

#[test]
fn sync_result_methods_and_unit_methods() {
    let rt = Runtime::new();
    rt.bind_foreign(<dyn SecureStore as Port>::PORT_ID, |method_id, args| {
        if method_id == id("SecureStore", "get") {
            return match String::decode_exact(args).unwrap().as_str() {
                "token" => Ok(Some(Bytes(vec![9])).encode_to_vec()),
                "missing" => Ok(None::<Bytes>.encode_to_vec()),
                _ => Err(PortError::Failed(HttpError::Timeout.encode_to_vec())),
            };
        }
        Ok(Vec::new())
    });
    let store = secure_store(&rt.ctx());
    assert_eq!(store.get("token".into()), Ok(Some(Bytes(vec![9]))));
    assert_eq!(store.get("missing".into()), Ok(None));
    assert_eq!(store.get("other".into()), Err(HttpError::Timeout));
    store.delete("token".into());
}

// ---------------------------------------------------------------------------------------------
// Rust bindings, `#[keel::port] impl`, and the Rust-side dispatcher
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct FakeHttp {
    calls: Mutex<Vec<String>>,
}

#[k::port]
impl Http for FakeHttp {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.calls.lock().unwrap().push(req.url.clone());
        if req.url == "bad" {
            return Err(HttpError::Timeout);
        }
        Ok(HttpResponse {
            status: 200,
            body: Bytes(req.url.into_bytes()),
        })
    }

    async fn ping(&self, url: String) -> bool {
        url == "up"
    }

    async fn fire(&self, msg: String) {
        self.calls.lock().unwrap().push(msg);
    }

    async fn check(&self, url: String) -> Result<(), HttpError> {
        if url == "ok" {
            Ok(())
        } else {
            Err(HttpError::Timeout)
        }
    }
}

fn bind_fake(rt: &Runtime) -> Arc<FakeHttp> {
    let fake = Arc::new(FakeHttp::default());
    let as_port: Arc<dyn Http> = fake.clone();
    rt.bind_port::<dyn Http>(<dyn Http as Port>::PORT_ID, Arc::new(as_port));
    fake
}

#[test]
fn the_accessor_prefers_a_rust_binding() {
    let (rt, ctx, seen) = runtime_with_host();
    let fake = bind_fake(&rt);
    let response = block_on(http(&ctx).request(HttpRequest {
        url: "direct".into(),
        retries: 0,
    }))
    .unwrap();
    assert_eq!(response.body, Bytes(b"direct".to_vec()));
    assert_eq!(*fake.calls.lock().unwrap(), ["direct"]);
    assert!(
        seen.lock().unwrap().is_empty(),
        "the host binding was not called"
    );
}

fn dispatch(imp: &Arc<dyn Http>, method: &str, args: &[u8]) -> Vec<u8> {
    match __keel_port_dispatch_Http(imp, id("Http", method), args) {
        PortDispatch::Sync(bytes) => bytes,
        PortDispatch::Async(future) => block_on(future),
    }
}

#[test]
fn the_rust_dispatcher_decodes_calls_and_encodes_replies_with_a_status_byte() {
    let fake: Arc<dyn Http> = Arc::new(FakeHttp::default());
    // Ok: status 0 then the response.
    let reply = dispatch(
        &fake,
        "request",
        &HttpRequest {
            url: "x".into(),
            retries: 1,
        }
        .encode_to_vec(),
    );
    assert_eq!(reply[0], 0);
    assert_eq!(
        HttpResponse::decode_exact(&reply[1..]).unwrap(),
        HttpResponse {
            status: 200,
            body: Bytes(b"x".to_vec())
        }
    );
    // Typed error: status 1 then the error.
    let reply = dispatch(
        &fake,
        "request",
        &HttpRequest {
            url: "bad".into(),
            retries: 0,
        }
        .encode_to_vec(),
    );
    assert_eq!(reply[0], 1);
    assert_eq!(
        HttpError::decode_exact(&reply[1..]).unwrap(),
        HttpError::Timeout
    );
    // Plain value and unit.
    let reply = dispatch(&fake, "ping", &"up".to_owned().encode_to_vec());
    assert_eq!(reply, [0, 1]);
    assert_eq!(
        dispatch(&fake, "fire", &"m".to_owned().encode_to_vec()),
        [0]
    );
    // `Result<(), E>`: ok has an empty body.
    assert_eq!(
        dispatch(&fake, "check", &"ok".to_owned().encode_to_vec()),
        [0]
    );
    let reply = dispatch(&fake, "check", &"no".to_owned().encode_to_vec());
    assert_eq!(reply[0], 1);
}

#[test]
fn the_rust_dispatcher_reports_unavailable_for_bad_requests() {
    let fake: Arc<dyn Http> = Arc::new(FakeHttp::default());
    assert_eq!(dispatch(&fake, "ping", &[]), [2]);
    let mut long = "x".to_owned().encode_to_vec();
    long.push(0);
    assert_eq!(dispatch(&fake, "ping", &long), [2]);
    assert_eq!(
        __keel_port_dispatch_Http(&fake, 0xdead_beef, &[]).into_sync(),
        [2]
    );
}

trait IntoSync {
    fn into_sync(self) -> Vec<u8>;
}

impl IntoSync for PortDispatch {
    fn into_sync(self) -> Vec<u8> {
        match self {
            PortDispatch::Sync(bytes) => bytes,
            PortDispatch::Async(_) => panic!("expected a synchronous dispatch"),
        }
    }
}

#[test]
fn dispatchers_are_registered_and_reachable_through_dyn_any() {
    let fake: Arc<dyn Http> = Arc::new(FakeHttp::default());
    let dispatcher = keel::meta::inventory::iter::<PortDispatcher>
        .into_iter()
        .find(|d| d.port_id == ids::port_id("Http"))
        .expect("Http dispatcher registered");
    let reply =
        match (dispatcher.dispatch)(&fake, id("Http", "ping"), &"up".to_owned().encode_to_vec()) {
            PortDispatch::Async(future) => block_on(future),
            PortDispatch::Sync(bytes) => bytes,
        };
    assert_eq!(reply, [0, 1]);
    // Something that is not an `Arc<dyn Http>` is unavailable.
    let wrong = 5_u32;
    assert_eq!(
        (dispatcher.dispatch)(&wrong, id("Http", "ping"), &[]).into_sync(),
        [2]
    );
}

#[test]
fn sync_dispatchers_reply_immediately() {
    struct FixedClock;
    #[k::port]
    impl Clock for FixedClock {
        fn now_ms(&self) -> i64 {
            7
        }
        fn monotonic_ns(&self) -> u64 {
            8
        }
    }
    let clock: Arc<dyn Clock> = Arc::new(FixedClock);
    let reply = __keel_port_dispatch_Clock(&clock, id("Clock", "now_ms"), &[]).into_sync();
    assert_eq!(reply[0], 0);
    assert_eq!(i64::decode_exact(&reply[1..]).unwrap(), 7);
}

// ---------------------------------------------------------------------------------------------
// Event ports
// ---------------------------------------------------------------------------------------------

#[test]
fn event_subscriptions_decode_the_payload() {
    let rt = Runtime::new();
    let ctx = rt.ctx();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let _subscription = on_connectivity_changed(&ctx, move |online, kind| {
        sink.lock().unwrap().push((online, kind));
    });
    let payload = encode_connectivity_changed_event(false, NetKind::Cellular);
    assert_eq!(
        payload,
        encode(|w| {
            false.encode(w);
            NetKind::Cellular.encode(w);
        })
    );
    rt.event(
        <dyn Connectivity as Port>::PORT_ID,
        id("Connectivity", "changed"),
        &payload,
    );
    assert_eq!(*seen.lock().unwrap(), [(false, NetKind::Cellular)]);

    // Malformed payloads and other ports' events are ignored.
    rt.event(
        <dyn Connectivity as Port>::PORT_ID,
        id("Connectivity", "changed"),
        &[9],
    );
    rt.event(
        <dyn Connectivity as Port>::PORT_ID,
        id("Connectivity", "changed"),
        &[0, 0, 0, 0, 0],
    );
    rt.event(
        <dyn Lifecycle as Port>::PORT_ID,
        id("Lifecycle", "changed"),
        &payload,
    );
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn event_methods_without_arguments_and_same_named_methods_do_not_clash() {
    let rt = Runtime::new();
    let ctx = rt.ctx();
    let states = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&states);
    let _a = on_lifecycle_changed(&ctx, move |state| sink.lock().unwrap().push(state));
    let count = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&count);
    let _b = on_lifecycle_low_memory(&ctx, move || *counter.lock().unwrap() += 1);
    let payload = encode_lifecycle_changed_event(2);
    rt.event(
        <dyn Lifecycle as Port>::PORT_ID,
        id("Lifecycle", "changed"),
        &payload,
    );
    let payload = encode_lifecycle_low_memory_event();
    assert!(payload.is_empty());
    rt.event(
        <dyn Lifecycle as Port>::PORT_ID,
        id("Lifecycle", "low_memory"),
        &payload,
    );
    assert_eq!(*states.lock().unwrap(), [2]);
    assert_eq!(*count.lock().unwrap(), 1);
}

// ---------------------------------------------------------------------------------------------
// Meta
// ---------------------------------------------------------------------------------------------

#[test]
fn port_consts_match_the_ids_and_kinds() {
    assert_eq!(<dyn Http as Port>::PORT_ID, ids::port_id("Http"));
    assert_eq!(<dyn Http as Port>::NAME, "Http");
    assert_eq!(<dyn Http as Port>::KIND, PortKind::Async);
    assert_eq!(<dyn Clock as Port>::KIND, PortKind::Sync);
    assert_eq!(
        <dyn Log as Port>::KIND,
        PortKind::Sync,
        "all-sync ports are Sync"
    );
    assert_eq!(<dyn Connectivity as Port>::KIND, PortKind::Event);
}

#[test]
fn port_meta_describes_every_method() {
    let schema = collect_schema("ports-test");
    let port = |name: &str| schema.ports.iter().find(|p| p.name == name).unwrap();
    let http = port("Http");
    assert_eq!(http.port_id, ids::port_id("Http"));
    assert_eq!(http.kind, PortKind::Async);
    assert_eq!(http.docs, "Talks HTTP.");
    let request = http.methods.iter().find(|m| m.name == "request").unwrap();
    assert_eq!(request.method_id, ids::port_method_id("Http", "request"));
    assert!(request.is_async && !request.takes_ctx);
    assert_eq!(request.docs, "Sends a request.");
    assert_eq!(request.params[0].name, "req");
    assert_eq!(request.params[0].ty, TypeRef::named("HttpRequest"));
    assert_eq!(
        request.returns,
        TypeRef::result(TypeRef::named("HttpResponse"), TypeRef::named("HttpError"))
    );
    let check = http.methods.iter().find(|m| m.name == "check").unwrap();
    assert_eq!(
        check.returns,
        TypeRef::result(TypeRef::Unit, TypeRef::named("HttpError"))
    );
    let fire = http.methods.iter().find(|m| m.name == "fire").unwrap();
    assert_eq!(fire.returns, TypeRef::Unit);

    let store = port("SecureStore");
    let get = store.methods.iter().find(|m| m.name == "get").unwrap();
    assert!(!get.is_async);
    assert_eq!(
        get.returns,
        TypeRef::result(TypeRef::option(TypeRef::Bytes), TypeRef::named("HttpError"))
    );
    let delete = store.methods.iter().find(|m| m.name == "delete").unwrap();
    assert_eq!(delete.params[0].name, "key");

    let events = port("Connectivity");
    assert_eq!(events.kind, PortKind::Event);
    assert_eq!(events.methods[0].params.len(), 2);
    assert_eq!(port("Clock").kind, PortKind::Sync);
}

#[test]
fn the_registered_schema_with_ports_validates() {
    collect_schema("ports-test")
        .validate()
        .unwrap_or_else(|errors| panic!("{errors:#?}"));
}
