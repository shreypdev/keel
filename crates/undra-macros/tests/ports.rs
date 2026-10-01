//! Behaviour tests for `#[undra::port]`: proxies, accessors, Rust-side dispatchers, event
//! subscriptions and the async-to-boxed-future rewrite, run against the real runtime.
#![forbid(unsafe_code)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use undra::meta::{PortKind, TypeRef, collect_schema, ids};
use undra::runtime::{Ctx, Port, PortDispatch, PortDispatcher, PortError};
use undra::wire::{Bytes, Decode, Encode, Reader, Writer};
use undra_macros as k;

mod support;
use support::Runtime;
use support::testing::block_on;

#[k::error]
#[derive(Clone, PartialEq)]
pub enum WebError {
    #[error("network: {0}")]
    Network(String),
    #[error("timeout")]
    Timeout,
}

/// A port that cannot answer (nobody registered it, the call was cancelled, the reply did not
/// decode) is an ordinary outcome; a method with an error channel reports it as its error.
impl From<PortError> for WebError {
    fn from(error: PortError) -> Self {
        WebError::Network(format!("port: {error}"))
    }
}

#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct WebRequest {
    pub url: String,
    pub retries: u8,
}

#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct WebResponse {
    pub status: u16,
    pub body: Bytes,
}

#[k::api]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LinkKind {
    Wifi,
    Cellular,
}

/// Talks HTTP.
#[k::port]
pub trait Web {
    /// Sends a request.
    async fn request(&self, req: WebRequest) -> Result<WebResponse, WebError>;
    async fn ping(&self, url: String) -> bool;
    async fn fire(&self, msg: String);
    async fn check(&self, url: String) -> Result<(), WebError>;
}

#[k::port(sync)]
pub trait Timekeeper {
    fn now_ms(&self) -> i64;
    fn monotonic_ns(&self) -> u64;
}

#[k::port]
pub trait Journal {
    fn log(&self, level: u8, target: String, message: String);
}

#[k::port]
pub trait Vault {
    fn get(&self, key: String) -> Result<Option<Bytes>, WebError>;
    fn delete(&self, key: String);
}

#[k::port(event)]
pub trait Reachability {
    fn changed(&self, online: bool, kind: LinkKind);
}

#[k::port(event)]
pub trait Phase {
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
    rt.bind_foreign(<dyn Web as Port>::PORT_ID, move |method_id, args| {
        if method_id == id("Web", "request") {
            let req = WebRequest::decode_exact(args).expect("args decode as WebRequest");
            log.lock().unwrap().push(format!("request {}", req.url));
            return match req.url.as_str() {
                "timeout" => Err(PortError::Failed(WebError::Timeout.encode_to_vec())),
                "garbage" => Ok(vec![1, 2, 3]),
                "gone" => Err(PortError::Unavailable),
                url => Ok(WebResponse {
                    status: 200 + u16::from(req.retries),
                    body: Bytes(url.as_bytes().to_vec()),
                }
                .encode_to_vec()),
            };
        }
        if method_id == id("Web", "ping") {
            let url = String::decode_exact(args).unwrap();
            log.lock().unwrap().push(format!("ping {url}"));
            if url == "garbage" {
                return Ok(vec![9, 9, 9]);
            }
            return Ok(true.encode_to_vec());
        }
        if method_id == id("Web", "fire") {
            let msg = String::decode_exact(args).unwrap();
            log.lock().unwrap().push(format!("fire {msg}"));
            return Ok(Vec::new());
        }
        if method_id == id("Web", "check") {
            let url = String::decode_exact(args).unwrap();
            return if url == "ok" {
                Ok(Vec::new())
            } else {
                Err(PortError::Failed(WebError::Network(url).encode_to_vec()))
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
    let http = web(&ctx);
    let response = block_on(http.request(WebRequest {
        url: "https://shreypdev.github.io/undra/".into(),
        retries: 2,
    }))
    .unwrap();
    assert_eq!(response.status, 202);
    assert_eq!(
        response.body,
        Bytes(b"https://shreypdev.github.io/undra/".to_vec())
    );
    assert_eq!(
        *seen.lock().unwrap(),
        ["request https://shreypdev.github.io/undra/"]
    );
}

#[test]
fn typed_errors_come_back_as_the_methods_error_type() {
    let (_rt, ctx, _) = runtime_with_host();
    let http = web(&ctx);
    let error = block_on(http.request(WebRequest {
        url: "timeout".into(),
        retries: 0,
    }))
    .unwrap_err();
    assert_eq!(error, WebError::Timeout);
    assert_eq!(block_on(http.check("ok".into())), Ok(()));
    assert_eq!(
        block_on(http.check("boom".into())),
        Err(WebError::Network("boom".into()))
    );
}

#[test]
fn plain_and_unit_returns() {
    let (_rt, ctx, seen) = runtime_with_host();
    let http = web(&ctx);
    assert!(block_on(http.ping("x".into())));
    block_on(http.fire("hello".into()));
    assert_eq!(*seen.lock().unwrap(), ["ping x", "fire hello"]);
}

#[test]
fn h2_a_method_without_an_error_channel_panics_with_a_message_that_teaches() {
    let rt = Runtime::new();
    let ctx = rt.ctx();
    let panic = catch_unwind(AssertUnwindSafe(|| {
        block_on(web(&ctx).ping("x".into()));
    }))
    .unwrap_err();
    let message = message_of(panic);
    for needle in [
        "undra: the `Web` port has no adapter registered (method `ping`)",
        "Register one with core.registerPort(..) (TypeScript, Kotlin, Swift) / undra_port_register (C)",
        "or bind a Rust implementation",
        "On the web this traps the core",
        "https://shreypdev.github.io/undra/docs/errors.html#E0062",
    ] {
        assert!(message.contains(needle), "missing `{needle}` in: {message}");
    }
}

#[test]
fn h2_an_undecodable_reply_of_an_infallible_method_names_the_port_and_method() {
    let (_rt, ctx, _) = runtime_with_host();
    let panic = catch_unwind(AssertUnwindSafe(|| {
        block_on(web(&ctx).ping("garbage".into()));
    }))
    .unwrap_err();
    let message = message_of(panic);
    assert!(
        message.contains("the `Web` port (method `ping`) replied with bytes that do not decode"),
        "{message}"
    );
}

#[test]
fn h2_result_methods_return_an_unavailable_port_as_their_error() {
    // No binding at all: SPEC 6.3 says a port nobody registered answers "unavailable".
    let rt = Runtime::new();
    let ctx = rt.ctx();
    let error = block_on(web(&ctx).request(WebRequest {
        url: "x".into(),
        retries: 0,
    }))
    .unwrap_err();
    assert_eq!(error, WebError::Network("port: port unavailable".into()));
    // A unit `Ok` type takes the same path.
    assert_eq!(
        block_on(web(&ctx).check("x".into())),
        Err(WebError::Network("port: port unavailable".into()))
    );
    // So does a synchronous port (`Vault::get` is not `async`).
    assert_eq!(
        vault(&ctx).get("k".into()),
        Err(WebError::Network("port: port unavailable".into()))
    );
}

#[test]
fn h2_result_methods_return_undecodable_replies_and_transport_failures_as_their_error() {
    let (_rt, ctx, _) = runtime_with_host();
    // `garbage` answers with bytes that are not a `WebResponse`.
    let error = block_on(web(&ctx).request(WebRequest {
        url: "garbage".into(),
        retries: 0,
    }))
    .unwrap_err();
    assert!(
        matches!(&error, WebError::Network(text) if text.starts_with("port: malformed port reply")),
        "{error:?}"
    );
    // A transport-level failure is the method's error as well, not a panic.
    let error = block_on(web(&ctx).request(WebRequest {
        url: "gone".into(),
        retries: 0,
    }))
    .unwrap_err();
    assert_eq!(error, WebError::Network("port: port unavailable".into()));
}

#[test]
fn h2_a_typed_error_the_adapter_reports_still_decodes_into_the_error_type() {
    let rt = Runtime::new();
    // The adapter reports a typed error whose bytes are not a `WebError`: that is a decode
    // failure, reported through `From<PortError>`, never a panic.
    rt.bind_foreign(<dyn Web as Port>::PORT_ID, |_, _| {
        Err(PortError::Failed(vec![0xff, 0xff]))
    });
    let error = block_on(web(&rt.ctx()).check("x".into())).unwrap_err();
    assert!(
        matches!(&error, WebError::Network(text) if text.starts_with("port: malformed port reply")),
        "{error:?}"
    );
}

#[test]
fn sync_ports_call_through_port_call_sync() {
    let rt = Runtime::new();
    rt.bind_foreign(<dyn Timekeeper as Port>::PORT_ID, |method_id, args| {
        assert!(args.is_empty());
        if method_id == id("Timekeeper", "now_ms") {
            Ok(1_700_000_000_000_i64.encode_to_vec())
        } else if method_id == id("Timekeeper", "monotonic_ns") {
            Ok(42_u64.encode_to_vec())
        } else {
            panic!("unexpected method");
        }
    });
    let clock = timekeeper(&rt.ctx());
    assert_eq!(clock.now_ms(), 1_700_000_000_000);
    assert_eq!(clock.monotonic_ns(), 42);
}

#[test]
fn fire_and_forget_sync_methods_ignore_the_reply() {
    let rt = Runtime::new();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    rt.bind_foreign(<dyn Journal as Port>::PORT_ID, move |_, args| {
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
    journal(&rt.ctx()).log(3, "core".into(), "hi".into());
    assert_eq!(*lines.lock().unwrap(), ["3 core hi"]);
}

#[test]
fn sync_result_methods_and_unit_methods() {
    let rt = Runtime::new();
    rt.bind_foreign(<dyn Vault as Port>::PORT_ID, |method_id, args| {
        if method_id == id("Vault", "get") {
            return match String::decode_exact(args).unwrap().as_str() {
                "token" => Ok(Some(Bytes(vec![9])).encode_to_vec()),
                "missing" => Ok(None::<Bytes>.encode_to_vec()),
                _ => Err(PortError::Failed(WebError::Timeout.encode_to_vec())),
            };
        }
        Ok(Vec::new())
    });
    let store = vault(&rt.ctx());
    assert_eq!(store.get("token".into()), Ok(Some(Bytes(vec![9]))));
    assert_eq!(store.get("missing".into()), Ok(None));
    assert_eq!(store.get("other".into()), Err(WebError::Timeout));
    store.delete("token".into());
}

// ---------------------------------------------------------------------------------------------
// Rust bindings, `#[undra::port] impl`, and the Rust-side dispatcher
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct FakeWeb {
    calls: Mutex<Vec<String>>,
}

#[k::port]
impl Web for FakeWeb {
    async fn request(&self, req: WebRequest) -> Result<WebResponse, WebError> {
        self.calls.lock().unwrap().push(req.url.clone());
        if req.url == "bad" {
            return Err(WebError::Timeout);
        }
        Ok(WebResponse {
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

    async fn check(&self, url: String) -> Result<(), WebError> {
        if url == "ok" {
            Ok(())
        } else {
            Err(WebError::Timeout)
        }
    }
}

fn bind_fake(rt: &Runtime) -> Arc<FakeWeb> {
    let fake = Arc::new(FakeWeb::default());
    let as_port: Arc<dyn Web> = fake.clone();
    rt.bind_port::<dyn Web>(<dyn Web as Port>::PORT_ID, Arc::new(as_port));
    fake
}

#[test]
fn the_accessor_prefers_a_rust_binding() {
    let (rt, ctx, seen) = runtime_with_host();
    let fake = bind_fake(&rt);
    let response = block_on(web(&ctx).request(WebRequest {
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

fn dispatch(imp: &Arc<dyn Web>, method: &str, args: &[u8]) -> Vec<u8> {
    match __undra_port_dispatch_Web(imp, id("Web", method), args) {
        PortDispatch::Sync(bytes) => bytes,
        PortDispatch::Async(future) => block_on(future),
    }
}

#[test]
fn the_rust_dispatcher_decodes_calls_and_encodes_replies_with_a_status_byte() {
    let fake: Arc<dyn Web> = Arc::new(FakeWeb::default());
    // Ok: status 0 then the response.
    let reply = dispatch(
        &fake,
        "request",
        &WebRequest {
            url: "x".into(),
            retries: 1,
        }
        .encode_to_vec(),
    );
    assert_eq!(reply[0], 0);
    assert_eq!(
        WebResponse::decode_exact(&reply[1..]).unwrap(),
        WebResponse {
            status: 200,
            body: Bytes(b"x".to_vec())
        }
    );
    // Typed error: status 1 then the error.
    let reply = dispatch(
        &fake,
        "request",
        &WebRequest {
            url: "bad".into(),
            retries: 0,
        }
        .encode_to_vec(),
    );
    assert_eq!(reply[0], 1);
    assert_eq!(
        WebError::decode_exact(&reply[1..]).unwrap(),
        WebError::Timeout
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
    let fake: Arc<dyn Web> = Arc::new(FakeWeb::default());
    assert_eq!(dispatch(&fake, "ping", &[]), [2]);
    let mut long = "x".to_owned().encode_to_vec();
    long.push(0);
    assert_eq!(dispatch(&fake, "ping", &long), [2]);
    assert_eq!(
        __undra_port_dispatch_Web(&fake, 0xdead_beef, &[]).into_sync(),
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
    let fake: Arc<dyn Web> = Arc::new(FakeWeb::default());
    let dispatcher = undra::meta::inventory::iter::<PortDispatcher>
        .into_iter()
        .find(|d| d.port_id == ids::port_id("Web"))
        .expect("Web dispatcher registered");
    let reply =
        match (dispatcher.dispatch)(&fake, id("Web", "ping"), &"up".to_owned().encode_to_vec()) {
            PortDispatch::Async(future) => block_on(future),
            PortDispatch::Sync(bytes) => bytes,
        };
    assert_eq!(reply, [0, 1]);
    // Something that is not an `Arc<dyn Web>` is unavailable.
    let wrong = 5_u32;
    assert_eq!(
        (dispatcher.dispatch)(&wrong, id("Web", "ping"), &[]).into_sync(),
        [2]
    );
}

#[test]
fn sync_dispatchers_reply_immediately() {
    struct FixedTimekeeper;
    #[k::port]
    impl Timekeeper for FixedTimekeeper {
        fn now_ms(&self) -> i64 {
            7
        }
        fn monotonic_ns(&self) -> u64 {
            8
        }
    }
    let clock: Arc<dyn Timekeeper> = Arc::new(FixedTimekeeper);
    let reply =
        __undra_port_dispatch_Timekeeper(&clock, id("Timekeeper", "now_ms"), &[]).into_sync();
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
    let _subscription = on_reachability_changed(&ctx, move |online, kind| {
        sink.lock().unwrap().push((online, kind));
    });
    let payload = encode_reachability_changed_event(false, LinkKind::Cellular);
    assert_eq!(
        payload,
        encode(|w| {
            false.encode(w);
            LinkKind::Cellular.encode(w);
        })
    );
    rt.event(
        <dyn Reachability as Port>::PORT_ID,
        id("Reachability", "changed"),
        &payload,
    );
    assert_eq!(*seen.lock().unwrap(), [(false, LinkKind::Cellular)]);

    // Malformed payloads and other ports' events are ignored.
    rt.event(
        <dyn Reachability as Port>::PORT_ID,
        id("Reachability", "changed"),
        &[9],
    );
    rt.event(
        <dyn Reachability as Port>::PORT_ID,
        id("Reachability", "changed"),
        &[0, 0, 0, 0, 0],
    );
    rt.event(
        <dyn Phase as Port>::PORT_ID,
        id("Phase", "changed"),
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
    let _a = on_phase_changed(&ctx, move |state| sink.lock().unwrap().push(state));
    let count = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&count);
    let _b = on_phase_low_memory(&ctx, move || *counter.lock().unwrap() += 1);
    let payload = encode_phase_changed_event(2);
    rt.event(
        <dyn Phase as Port>::PORT_ID,
        id("Phase", "changed"),
        &payload,
    );
    let payload = encode_phase_low_memory_event();
    assert!(payload.is_empty());
    rt.event(
        <dyn Phase as Port>::PORT_ID,
        id("Phase", "low_memory"),
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
    assert_eq!(<dyn Web as Port>::PORT_ID, ids::port_id("Web"));
    assert_eq!(<dyn Web as Port>::NAME, "Web");
    assert_eq!(<dyn Web as Port>::KIND, PortKind::Async);
    assert_eq!(<dyn Timekeeper as Port>::KIND, PortKind::Sync);
    assert_eq!(
        <dyn Journal as Port>::KIND,
        PortKind::Sync,
        "all-sync ports are Sync"
    );
    assert_eq!(<dyn Reachability as Port>::KIND, PortKind::Event);
}

#[test]
fn port_meta_describes_every_method() {
    let schema = collect_schema("ports-test");
    let port = |name: &str| schema.ports.iter().find(|p| p.name == name).unwrap();
    let http = port("Web");
    assert_eq!(http.port_id, ids::port_id("Web"));
    assert_eq!(http.kind, PortKind::Async);
    assert_eq!(http.docs, "Talks HTTP.");
    let request = http.methods.iter().find(|m| m.name == "request").unwrap();
    assert_eq!(request.method_id, ids::port_method_id("Web", "request"));
    assert!(request.is_async && !request.takes_ctx);
    assert_eq!(request.docs, "Sends a request.");
    assert_eq!(request.params[0].name, "req");
    assert_eq!(request.params[0].ty, TypeRef::named("WebRequest"));
    assert_eq!(
        request.returns,
        TypeRef::result(TypeRef::named("WebResponse"), TypeRef::named("WebError"))
    );
    let check = http.methods.iter().find(|m| m.name == "check").unwrap();
    assert_eq!(
        check.returns,
        TypeRef::result(TypeRef::Unit, TypeRef::named("WebError"))
    );
    let fire = http.methods.iter().find(|m| m.name == "fire").unwrap();
    assert_eq!(fire.returns, TypeRef::Unit);

    let store = port("Vault");
    let get = store.methods.iter().find(|m| m.name == "get").unwrap();
    assert!(!get.is_async);
    assert_eq!(
        get.returns,
        TypeRef::result(TypeRef::option(TypeRef::Bytes), TypeRef::named("WebError"))
    );
    let delete = store.methods.iter().find(|m| m.name == "delete").unwrap();
    assert_eq!(delete.params[0].name, "key");

    let events = port("Reachability");
    assert_eq!(events.kind, PortKind::Event);
    assert_eq!(events.methods[0].params.len(), 2);
    assert_eq!(port("Timekeeper").kind, PortKind::Sync);
}

#[test]
fn the_registered_schema_with_ports_validates() {
    collect_schema("ports-test")
        .validate()
        .unwrap_or_else(|errors| panic!("{errors:#?}"));
}
