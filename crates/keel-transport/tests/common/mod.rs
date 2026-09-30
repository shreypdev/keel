//! Shared by the integration tests: a small core written with the real macros (a store with
//! sync, async and streaming methods and a platform port), a server around it, and a raw
//! WebSocket client that behaves the way a platform's `remote` transport does.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use keel::meta::ids;
use keel::prelude::*;
use keel::runtime::testing::call_payload;
use keel::runtime::{Runtime, RuntimeConfig, Stream};
use keel::wire::payload::{
    CallTarget, Cancel, Hello, Observe, PortReply, PortStatus, Release, Reply, ReplyStatus,
    StreamCredit, StreamFlag, StreamItem,
};
use keel::wire::{Decode, Encode, Envelope, Kind, Reader, Writer};
use keel_transport::{Bridge, Server, ServerConfig};
use tungstenite::{Message, WebSocket};

/// Runs `f` on the runtime's core (as a spawned task) and waits for it: the way an embedding app
/// writes signals into its own core. A write from the app's own thread would not hold the core
/// lock, which the runtime's write-context check refuses in debug builds (ADR-023).
pub fn on_core<R: Send + 'static>(rt: &Runtime, f: impl FnOnce() -> R + Send + 'static) -> R {
    let (tx, rx) = std::sync::mpsc::channel();
    rt.ctx().spawn(async move {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("the task ran on the core")
}

// ----- the core under test -------------------------------------------------------------------

/// Counts how often each of the fixture's futures and streams was dropped, which is how the
/// tests see a cancellation happen inside the core.
pub static DROPPED: AtomicUsize = AtomicUsize::new(0);
/// Counts how often `hang` was started.
pub static STARTED: AtomicUsize = AtomicUsize::new(0);
/// What detached port calls (`ask_later`) came back with.
pub static LATE: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct DropGuard;

impl Drop for DropGuard {
    fn drop(&mut self) {
        DROPPED.fetch_add(1, Ordering::SeqCst);
    }
}

#[keel::error]
#[derive(Clone, Debug, PartialEq)]
pub enum CounterError {
    #[error("{0} is negative")]
    Negative(i32),
}

/// What the client platform implements.
#[keel::port]
pub trait Echo {
    async fn echo(&self, x: i32) -> i32;
}

/// A synchronous port: a remote client cannot answer it in time.
#[keel::port(sync)]
pub trait Wall {
    fn now(&self) -> i64;
}

/// A synchronous fire-and-forget port (like `Log`).
#[keel::port(sync)]
pub trait Beep {
    fn beep(&self, n: u32);
}

#[keel::store]
pub struct Counter {
    ctx: Ctx,
    count: Signal<i32>,
    label: Signal<String>,
}

#[keel::api(store)]
impl Counter {
    pub fn new(ctx: Ctx, initial: i32) -> Self {
        Self {
            ctx,
            count: Signal::new(initial),
            label: Signal::new(String::new()),
        }
    }

    pub fn get(&self) -> i32 {
        self.count.get()
    }

    pub fn add(&self, n: i32) -> i32 {
        self.count.update(|c| *c += n);
        self.count.get()
    }

    /// Two writes in one transaction: one change-set.
    pub fn add_and_label(&self, n: i32) {
        self.ctx.txn(|| {
            self.count.update(|c| *c += n);
            self.label.set(format!("n={n}"));
        });
    }

    pub fn check(&self, n: i32) -> Result<i32, CounterError> {
        if n < 0 {
            Err(CounterError::Negative(n))
        } else {
            Ok(n)
        }
    }

    pub fn boom(&self) {
        panic!("kaboom");
    }

    pub async fn slow_add(&self, n: i32) -> i32 {
        self.ctx.sleep(Duration::from_millis(20)).await;
        self.count.update(|c| *c += n);
        self.count.get()
    }

    /// Never finishes; its future holds a guard so the test can see it dropped.
    pub async fn hang(&self) {
        let _guard = DropGuard;
        STARTED.fetch_add(1, Ordering::SeqCst);
        std::future::pending::<()>().await;
    }

    /// Asks the platform.
    pub async fn ask(&self, x: i32) -> i32 {
        echo(&self.ctx).echo(x).await
    }

    /// Asks the platform from a detached task, no call waiting on it; the outcome is
    /// recorded in [`LATE`].
    pub fn ask_later(&self, x: i32) {
        let ctx = self.ctx.clone();
        self.ctx.spawn(async move {
            let outcome = match ctx
                .port_call(ECHO_PORT, ECHO_METHOD, x.encode_to_vec())
                .await
            {
                Ok(body) => format!("ok {}", i32::decode_exact(&body).unwrap()),
                Err(e) => format!("err {e}"),
            };
            LATE.lock().unwrap().push(outcome);
        });
    }

    pub fn wall_now(&self) -> i64 {
        wall(&self.ctx).now()
    }

    pub fn beep_twice(&self) -> u32 {
        beep(&self.ctx).beep(1);
        beep(&self.ctx).beep(2);
        2
    }

    pub fn ticks(&self, n: u32) -> impl Stream<Item = u32> + Send + 'static {
        Ticks {
            next: 0,
            end: n,
            guard: None,
        }
    }

    pub fn endless(&self) -> impl Stream<Item = u32> + Send + 'static {
        Ticks {
            next: 0,
            end: u32::MAX,
            guard: Some(DropGuard),
        }
    }
}

struct Ticks {
    next: u32,
    end: u32,
    guard: Option<DropGuard>,
}

impl Stream for Ticks {
    type Item = u32;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<u32>> {
        let _keep = &self.guard;
        if self.next >= self.end {
            return Poll::Ready(None);
        }
        let item = self.next;
        self.next += 1;
        Poll::Ready(Some(item))
    }
}

#[keel::api]
pub fn sum(a: i32, b: i32) -> i32 {
    a + b
}

#[keel::api]
pub fn echo_bytes(data: Bytes) -> Bytes {
    data
}

pub const NEW: u32 = ids::method_id("Counter", "new");
pub const GET: u32 = ids::method_id("Counter", "get");
pub const ADD: u32 = ids::method_id("Counter", "add");
pub const ADD_AND_LABEL: u32 = ids::method_id("Counter", "add_and_label");
pub const CHECK: u32 = ids::method_id("Counter", "check");
pub const BOOM: u32 = ids::method_id("Counter", "boom");
pub const SLOW_ADD: u32 = ids::method_id("Counter", "slow_add");
pub const HANG: u32 = ids::method_id("Counter", "hang");
pub const ASK: u32 = ids::method_id("Counter", "ask");
pub const TICKS: u32 = ids::method_id("Counter", "ticks");
pub const ASK_LATER: u32 = ids::method_id("Counter", "ask_later");
pub const WALL_NOW: u32 = ids::method_id("Counter", "wall_now");
pub const BEEP_TWICE: u32 = ids::method_id("Counter", "beep_twice");
pub const WALL_PORT: u32 = ids::port_id("Wall");
pub const BEEP_PORT: u32 = ids::port_id("Beep");
pub const ENDLESS: u32 = ids::method_id("Counter", "endless");
pub const SUM: u32 = ids::function_id("sum");
pub const ECHO_BYTES: u32 = ids::function_id("echo_bytes");
pub const COUNTER: u32 = ids::type_id("Counter");
pub const ECHO_PORT: u32 = ids::port_id("Echo");
pub const ECHO_METHOD: u32 = ids::port_method_id("Echo", "echo");
pub const COUNT_SIGNAL: u32 = 0;

static SERIAL: Mutex<()> = Mutex::new(());

/// Serialises the tests that read the `DROPPED` / `STARTED` counters, which every test in a
/// binary shares.
pub fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn enc<T: Encode + ?Sized>(value: &T) -> Vec<u8> {
    value.encode_to_vec()
}

pub fn dec<T: Decode>(bytes: &[u8]) -> T {
    T::decode_exact(bytes).expect("the body decodes")
}

// ----- a server around it ----------------------------------------------------------------------

/// The log records the runtime and the server produced, as the bridge's sink saw them.
pub type Logs = Arc<Mutex<Vec<(u8, String, String)>>>;

pub struct Fixture {
    pub server: Server,
    pub rt: Arc<Runtime>,
    pub bridge: Arc<Bridge>,
    pub logs: Logs,
}

/// Timeouts short enough that a hung test fails quickly instead of stalling the suite.
pub fn quick() -> ServerConfig {
    ServerConfig {
        handshake_timeout: Duration::from_secs(3),
        close_timeout: Duration::from_millis(500),
        write_timeout: Duration::from_secs(5),
        busy_grace: Duration::from_millis(400),
        ..ServerConfig::default()
    }
}

pub fn start() -> Fixture {
    start_with(quick(), "dev")
}

pub fn start_with(config: ServerConfig, mode: &str) -> Fixture {
    let mode = mode.to_owned();
    let server = Server::start("127.0.0.1:0", config, move |host| {
        Runtime::new(
            RuntimeConfig {
                platform: "rust".into(),
                mode,
                core_threads: 1,
                blocking_threads: 1,
                log_level: 0,
            },
            host,
        )
    })
    .expect("the server starts");
    let logs: Logs = Arc::new(Mutex::new(Vec::new()));
    let sink = logs.clone();
    server.bridge().set_log_sink(move |level, target, message| {
        sink.lock()
            .unwrap()
            .push((level, target.to_owned(), message.to_owned()));
    });
    Fixture {
        rt: server.runtime().clone(),
        bridge: server.bridge().clone(),
        server,
        logs,
    }
}

impl Fixture {
    pub fn url(&self) -> String {
        self.server.url()
    }

    pub fn schema(&self) -> u64 {
        self.rt.schema_hash()
    }

    /// A client that has completed the handshake as a `"test"` platform in dev mode.
    pub fn client(&self) -> TestClient {
        TestClient::connect(&self.url(), self.schema())
    }

    pub fn log_lines(&self) -> Vec<String> {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .map(|(level, target, message)| format!("{level} {target}: {message}"))
            .collect()
    }

    /// Waits until `condition` holds, panicking with `what` if it does not within 5 seconds.
    pub fn eventually(&self, what: &str, condition: impl Fn(&Fixture) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition(self) {
            assert!(Instant::now() < deadline, "timed out waiting for: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.shutdown();
        self.rt.shutdown();
    }
}

/// One number out of the runtime's `stats_json`.
pub fn stat(rt: &Runtime, key: &str) -> u64 {
    let stats: serde_json::Value = serde_json::from_str(&rt.stats_json()).unwrap();
    stats
        .get(key)
        .or_else(|| stats["crossings"].get(key))
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| panic!("no stat {key}"))
}

pub fn eventually(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ----- a raw client ------------------------------------------------------------------------------

/// One envelope received from the server.
#[derive(Clone, Debug)]
pub struct Frame {
    pub kind: Kind,
    pub seq: u32,
    pub schema: u64,
    pub payload: Vec<u8>,
}

/// What `recv` saw.
#[derive(Debug)]
pub enum Received {
    Frame(Frame),
    /// The server closed: the code and reason of its Close frame, if it sent one.
    Closed(Option<(u16, String)>),
    /// Nothing arrived in time.
    Silence,
}

pub struct TestClient {
    ws: WebSocket<TcpStream>,
    /// The schema hash the client stamps on its envelopes.
    pub schema: u64,
    seq: u32,
    /// Everything received so far, in order (`recv_kind` keeps what it skips here too).
    pub seen: Vec<Frame>,
    pub hello: Option<Frame>,
    call_id: u32,
    /// Replies that arrived while waiting for another call's.
    replies: std::collections::HashMap<u32, (ReplyStatus, Vec<u8>)>,
}

impl TestClient {
    /// Connects the socket only: no Hello.
    pub fn connect_raw(url: &str, schema: u64) -> TestClient {
        TestClient::connect_with_origin(url, schema, None)
    }

    /// Like [`connect_raw`](TestClient::connect_raw), presenting `origin` as a browser would.
    pub fn connect_with_origin(url: &str, schema: u64, origin: Option<&str>) -> TestClient {
        use tungstenite::client::IntoClientRequest;
        let host = url.trim_start_matches("ws://");
        let tcp = TcpStream::connect(host).expect("the server accepts");
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        tcp.set_nodelay(true).unwrap();
        let mut request = url.into_client_request().unwrap();
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert("Origin", origin.parse().unwrap());
        }
        let (ws, _response) =
            tungstenite::client(request, tcp).expect("the WebSocket upgrade succeeds");
        TestClient {
            ws,
            schema,
            seq: 0,
            seen: Vec::new(),
            hello: None,
            call_id: 0,
            replies: std::collections::HashMap::new(),
        }
    }

    /// Connects and completes the handshake as `platform = "test"`, `mode = "dev"`.
    pub fn connect(url: &str, schema: u64) -> TestClient {
        let mut client = TestClient::connect_raw(url, schema);
        client.handshake("test", "dev");
        client
    }

    /// Sends a Hello and expects the server's back, with the same schema.
    pub fn handshake(&mut self, platform: &str, mode: &str) -> Frame {
        self.send_hello(self.schema, platform, mode);
        let hello = self.expect_frame(Kind::Hello);
        let parsed = Hello::decode(&mut Reader::new(&hello.payload)).unwrap();
        assert_eq!(parsed.schema_hash, self.schema, "the server's schema hash");
        self.hello = Some(hello.clone());
        hello
    }

    pub fn send_hello(&mut self, schema_in_payload: u64, platform: &str, mode: &str) {
        let mut w = Writer::new();
        Hello {
            keel_version: "0.0.0-test",
            schema_hash: schema_in_payload,
            platform,
            mode,
        }
        .encode(&mut w);
        self.send(Kind::Hello, w.as_slice());
    }

    /// Where this client's sequence numbers start (TypeScript and Kotlin 0, Swift 1).
    pub fn start_seq_at(&mut self, seq: u32) {
        self.seq = seq;
    }

    pub fn send(&mut self, kind: Kind, payload: &[u8]) {
        let mut w = Writer::new();
        Envelope::write(&mut w, kind, self.seq, self.schema, payload);
        self.seq = self.seq.wrapping_add(1);
        self.send_binary(w.into_vec());
    }

    /// Sends bytes as one binary message, whatever they are.
    pub fn send_binary(&mut self, bytes: Vec<u8>) {
        self.ws
            .send(Message::Binary(bytes))
            .expect("the send succeeds");
    }

    /// Like [`send`](TestClient::send) but reports a broken connection instead of panicking.
    pub fn try_send(&mut self, kind: Kind, payload: &[u8]) -> bool {
        let mut w = Writer::new();
        Envelope::write(&mut w, kind, self.seq, self.schema, payload);
        self.seq = self.seq.wrapping_add(1);
        self.ws.send(Message::Binary(w.into_vec())).is_ok()
    }

    pub fn send_text(&mut self, text: &str) {
        self.ws
            .send(Message::Text(text.into()))
            .expect("the send succeeds");
    }

    pub fn ws(&mut self) -> &mut WebSocket<TcpStream> {
        &mut self.ws
    }

    pub fn next_call_id(&mut self) -> u32 {
        self.call_id += 1;
        self.call_id
    }

    // ----- calls ---------------------------------------------------------------------------------

    pub fn send_call(&mut self, target: CallTarget, call_id: u32, args: &[u8]) {
        self.send(Kind::Call, &call_payload(target, call_id, args));
    }

    /// Calls and waits for the `Reply` of that call.
    pub fn call(&mut self, target: CallTarget, args: &[u8]) -> (ReplyStatus, Vec<u8>) {
        let id = self.next_call_id();
        self.send_call(target, id, args);
        self.await_reply(id)
    }

    /// The reply to `call_id`, whether it already arrived or is yet to.
    pub fn await_reply(&mut self, call_id: u32) -> (ReplyStatus, Vec<u8>) {
        loop {
            if let Some(reply) = self.replies.remove(&call_id) {
                return reply;
            }
            let frame = self.expect_any();
            if frame.kind == Kind::Reply {
                let reply = Reply::decode(&mut Reader::new(&frame.payload)).unwrap();
                self.replies
                    .insert(reply.call_id, (reply.status, reply.body.to_vec()));
            }
        }
    }

    /// Constructs a `Counter` and returns its handle.
    pub fn new_counter(&mut self, initial: i32) -> u64 {
        let (status, body) = self.call(
            CallTarget::Constructor {
                type_id: COUNTER,
                method_id: NEW,
            },
            &enc(&initial),
        );
        assert_eq!(status, ReplyStatus::Ok);
        dec::<u64>(&body)
    }

    pub fn method(&mut self, handle: u64, method_id: u32, args: &[u8]) -> (ReplyStatus, Vec<u8>) {
        self.call(
            CallTarget::Method {
                handle: keel::wire::Handle(handle),
                method_id,
            },
            args,
        )
    }

    pub fn observe(&mut self, handle: u64, signal_id: u32, on: bool) {
        let mut w = Writer::new();
        Observe {
            handle: keel::wire::Handle(handle),
            signal_id,
            on,
        }
        .encode(&mut w);
        self.send(Kind::Observe, w.as_slice());
    }

    pub fn release(&mut self, handle: u64) {
        let mut w = Writer::new();
        Release {
            handle: keel::wire::Handle(handle),
        }
        .encode(&mut w);
        self.send(Kind::Release, w.as_slice());
    }

    pub fn cancel(&mut self, call_id: u32) {
        let mut w = Writer::new();
        Cancel { call_id }.encode(&mut w);
        self.send(Kind::Cancel, w.as_slice());
    }

    pub fn credit(&mut self, call_id: u32, credit: u32) {
        let mut w = Writer::new();
        StreamCredit { call_id, credit }.encode(&mut w);
        self.send(Kind::StreamCredit, w.as_slice());
    }

    pub fn port_reply(&mut self, port_call_id: u32, status: PortStatus, body: &[u8]) {
        let mut w = Writer::new();
        PortReply {
            port_call_id,
            status,
            body,
        }
        .encode(&mut w);
        self.send(Kind::PortReply, w.as_slice());
    }

    // ----- receiving --------------------------------------------------------------------------------

    /// The next message, a close or silence after `timeout`.
    pub fn recv_within(&mut self, timeout: Duration) -> Received {
        let _ = self.ws.get_ref().set_read_timeout(Some(timeout));
        loop {
            match self.ws.read() {
                Ok(Message::Binary(bytes)) => {
                    let env = Envelope::parse(&bytes).expect("the server sends valid envelopes");
                    let frame = Frame {
                        kind: env.kind,
                        seq: env.seq,
                        schema: env.schema,
                        payload: env.payload.to_vec(),
                    };
                    self.seen.push(frame.clone());
                    return Received::Frame(frame);
                }
                Ok(Message::Close(frame)) => {
                    let _ = self.ws.flush();
                    return Received::Closed(
                        frame.map(|f| (u16::from(f.code), f.reason.into_owned())),
                    );
                }
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                Ok(Message::Text(text)) => panic!("the server sent a text message: {text}"),
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Received::Silence;
                }
                Err(_) => return Received::Closed(None),
            }
        }
    }

    pub fn recv(&mut self) -> Received {
        self.recv_within(Duration::from_secs(5))
    }

    /// The next envelope of any kind; panics on a close or silence.
    pub fn expect_any(&mut self) -> Frame {
        match self.recv() {
            Received::Frame(frame) => frame,
            other => panic!("expected an envelope, got {other:?}"),
        }
    }

    /// The next envelope, which must be of `kind`.
    pub fn expect_frame(&mut self, kind: Kind) -> Frame {
        let frame = self.expect_any();
        assert_eq!(frame.kind, kind, "unexpected {:?}", frame);
        frame
    }

    /// The next envelope of `kind`, skipping (but remembering) others such as Log.
    pub fn recv_kind(&mut self, kind: Kind) -> Frame {
        loop {
            let frame = self.expect_any();
            if frame.kind == kind {
                return frame;
            }
        }
    }

    /// Waits for the server to close and returns the close code and reason (`None` for a
    /// close without a Close frame). Envelopes that arrive first are kept in `seen`.
    pub fn expect_close(&mut self) -> Option<(u16, String)> {
        loop {
            match self.recv() {
                Received::Frame(_) => {}
                Received::Closed(close) => return close,
                Received::Silence => panic!("the server did not close"),
            }
        }
    }

    /// Whether anything at all arrives in `timeout` (a frame or a close).
    /// Whether no frame but dev-mode `Log` chatter arrives within `timeout`.
    ///
    /// A core in `mode = "dev"` may log at any moment (SPEC 5.10: every port call and
    /// transaction), and background work such as keel-query's hydration may offer this
    /// client a `PortCall` whenever it runs. Silence is about frames addressed to *this
    /// client's calls*: `Log` and `PortCall` restart the wait, anything else ends it.
    pub fn silent_for(&mut self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return true;
            }
            match self.recv_within(left) {
                Received::Silence => return true,
                Received::Frame(frame) if matches!(frame.kind, Kind::Log | Kind::PortCall) => {
                    continue;
                }
                Received::Frame(_) | Received::Closed(_) => return false,
            }
        }
    }

    pub fn frames_of(&self, kind: Kind) -> Vec<&Frame> {
        self.seen.iter().filter(|f| f.kind == kind).collect()
    }
}

/// The change-set in `frame`, decoded.
pub fn change_set(frame: &Frame) -> keel::wire::payload::ChangeSet {
    keel::wire::payload::ChangeSet::decode(&mut Reader::new(&frame.payload)).unwrap()
}

/// Every change-set received so far, decoded, in arrival order.
pub fn change_sets(client: &TestClient) -> Vec<keel::wire::payload::ChangeSet> {
    client
        .frames_of(Kind::ChangeSet)
        .into_iter()
        .map(change_set)
        .collect()
}

pub fn stream_item(frame: &Frame) -> (u32, StreamFlag, Vec<u8>) {
    let item = StreamItem::decode(&mut Reader::new(&frame.payload)).unwrap();
    (item.call_id, item.flag, item.body.to_vec())
}

// ----- a hand-rolled WebSocket client, for framing tungstenite's client will not produce ---------

/// A minimal RFC 6455 client: the upgrade, then whatever frames a test builds by hand.
pub struct RawWs {
    pub tcp: TcpStream,
}

/// One frame the server sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFrame {
    pub fin: bool,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

impl RawWs {
    /// Connects and performs the upgrade; panics unless the server answers `101`.
    pub fn connect(addr: std::net::SocketAddr) -> RawWs {
        let (ws, head) = RawWs::upgrade(addr, &[]);
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        ws
    }

    /// Sends the upgrade request with `headers` added and returns the server's answer (the
    /// status line and headers, whatever the status is).
    pub fn upgrade(addr: std::net::SocketAddr, headers: &[(&str, &str)]) -> (RawWs, String) {
        let mut ws = RawWs::connect_socket(addr);
        let extra: String = headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect();
        ws.tcp
            .write_all(
                format!(
                    "GET / HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
                     Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n{extra}\r\n"
                )
                .as_bytes(),
            )
            .unwrap();
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            ws.tcp.read_exact(&mut byte).expect("the upgrade response");
            head.push(byte[0]);
        }
        (ws, String::from_utf8_lossy(&head).into_owned())
    }

    /// Connects the socket without upgrading.
    pub fn connect_socket(addr: std::net::SocketAddr) -> RawWs {
        let tcp = TcpStream::connect(addr).expect("the server accepts");
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        tcp.set_nodelay(true).unwrap();
        RawWs { tcp }
    }

    /// Writes one frame. `mask` false sends it unmasked, which a client must never do.
    pub fn frame(&mut self, first_byte: u8, payload: &[u8], mask: bool) {
        let mut out = vec![first_byte];
        let flag = if mask { 0x80 } else { 0 };
        match payload.len() {
            n if n < 126 => out.push(flag | n as u8),
            n if n < 65_536 => {
                out.push(flag | 126);
                out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                out.push(flag | 127);
                out.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        if mask {
            let key = [0x12_u8, 0x34, 0x56, 0x78];
            out.extend_from_slice(&key);
            out.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
        } else {
            out.extend_from_slice(payload);
        }
        self.tcp.write_all(&out).unwrap();
    }

    /// A masked, final binary frame.
    pub fn binary(&mut self, payload: &[u8]) {
        self.frame(0x82, payload, true);
    }

    /// Reads server frames until the connection ends or `limit` passes.
    pub fn frames_until_end(&mut self, limit: Duration) -> Vec<RawFrame> {
        let deadline = Instant::now() + limit;
        let mut frames = Vec::new();
        while Instant::now() < deadline {
            let mut head = [0_u8; 2];
            if self.tcp.read_exact(&mut head).is_err() {
                break;
            }
            let (fin, opcode) = (head[0] & 0x80 != 0, head[0] & 0x0f);
            let mut len = u64::from(head[1] & 0x7f);
            if len == 126 {
                let mut b = [0_u8; 2];
                if self.tcp.read_exact(&mut b).is_err() {
                    break;
                }
                len = u64::from(u16::from_be_bytes(b));
            } else if len == 127 {
                let mut b = [0_u8; 8];
                if self.tcp.read_exact(&mut b).is_err() {
                    break;
                }
                len = u64::from_be_bytes(b);
            }
            let mut payload = vec![0_u8; len as usize];
            if self.tcp.read_exact(&mut payload).is_err() {
                break;
            }
            frames.push(RawFrame {
                fin,
                opcode,
                payload,
            });
        }
        frames
    }

    /// The code of the Close frame among `frames`, if any.
    pub fn close_code(frames: &[RawFrame]) -> Option<u16> {
        frames.iter().find(|f| f.opcode == 8).map(|f| {
            if f.payload.len() >= 2 {
                u16::from_be_bytes([f.payload[0], f.payload[1]])
            } else {
                1005
            }
        })
    }
}

/// A small deterministic PRNG for the byte-fuzz tests (xorshift64*).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    /// Up to `max` (exclusive) random bytes.
    pub fn junk(&mut self, max: usize) -> Vec<u8> {
        let len = self.below(max);
        self.bytes(len)
    }

    pub fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next() as u8).collect()
    }
}
