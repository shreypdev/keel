//! `undra dev` carries the core's state across a rebuild (ADR-053): the real `undra dev` on a
//! copy of the playground, a raw WebSocket client that behaves the way a platform runtime's
//! `remote` transport does (a session token, resume, observe), edits of the core's sources, and
//! what the client finds afterwards.

mod common;

use std::io::Write;
use std::net::TcpStream;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::Duration;

use common::devserver::Dev;
use common::playground_copy;
use tungstenite::{Message, WebSocket};
use undra_meta::ids;
use undra_wire::payload::{
    Call, CallTarget, ChangeSet, Hello, LazyPage, LazyValue, Log, Observe, Reply, ReplyStatus,
};
use undra_wire::{Decode, Encode, Envelope, Handle, Kind, Reader, Writer};

const BUILD: Duration = Duration::from_secs(900);

/// How long [`Client::read`] waits for the server before it calls it silence.
const PATIENCE: Duration = Duration::from_secs(30);

/// How often the connection's thread comes up from the socket to see whether the test has
/// something to send.
const TICK: Duration = Duration::from_millis(10);

/// What a read from the server produced.
enum Got {
    Frame(Kind, Vec<u8>),
    Closed(Option<(u16, String)>),
    Silence,
}

/// What the connection's thread found on the socket: a message, or the error that ended it.
type Inbound = Result<Message, tungstenite::Error>;

/// Services the socket of a [`Client`] for as long as the client lives, the way the reader of a
/// platform runtime does: whatever the test sends goes out, and everything that comes in is queued
/// for the test, **while the test is busy with something else**.
///
/// That is the point of the thread. The server drops a client that has not answered a Ping within
/// three keepalive intervals (`ServerConfig::ping_interval`, 5 s: 15 s) as dead, without a Close
/// frame, and a client answers a Ping only when it reads. A test that waits for a rebuild reads
/// nothing for as long as the build takes, which under load is longer than 15 s: the server then
/// dropped the client mid-test and the test saw a reset instead of the reload's Close (1001). Real
/// clients never read in bursts, so the harness does not either.
fn pump(mut ws: WebSocket<TcpStream>, outbound: &Receiver<Vec<u8>>, inbound: &Sender<Inbound>) {
    loop {
        loop {
            match outbound.try_recv() {
                Ok(bytes) => {
                    if let Err(error) = ws.send(Message::Binary(bytes)) {
                        let _ = inbound.send(Err(error));
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                // The client was dropped: closing the socket is how a test hangs up.
                Err(TryRecvError::Disconnected) => return,
            }
        }
        // Answers a Ping by itself (the Pong goes out on the next read), within `TICK`.
        match ws.read() {
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            Ok(Message::Close(frame)) => {
                // The echo of the server's Close, then the end of the connection.
                let _ = ws.flush();
                let _ = inbound.send(Ok(Message::Close(frame)));
                return;
            }
            Ok(message) => {
                if inbound.send(Ok(message)).is_err() {
                    return;
                }
            }
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => {
                let _ = inbound.send(Err(error));
                return;
            }
        }
    }
}

/// A client of `undra dev` speaking the envelope by hand, with a session token as the platform
/// runtimes send (ADR-051).
struct Client {
    /// Envelopes for the connection's thread to send, in order.
    outbound: Sender<Vec<u8>>,
    /// What that thread read, in order; disconnected once the connection has ended.
    inbound: Receiver<Inbound>,
    /// How long [`Client::read`] waits.
    patience: Duration,
    schema: u64,
    seq: u32,
    next_call: u32,
    /// The `undra::dev` sentences received so far.
    notices: Vec<String>,
    /// The hash in the server's Hello.
    server_schema: u64,
    /// The latest value received for each `(store handle, signal id)`.
    values: std::collections::HashMap<(u64, u32), Vec<u8>>,
    /// The close [`Client::reply_or_close`] met, if it met one.
    closed: Option<(u16, String)>,
}

impl Client {
    /// Connects, sends `Hello` (stamping `schema`) and reads the server's.
    fn connect(dev: &Dev, schema: u64, token: &str, resume: bool) -> Client {
        let tcp = TcpStream::connect(dev.addr()).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let url = format!(
            "{}/?undra_session={token}{}",
            dev.url.trim_end_matches('/'),
            if resume { "&undra_resume=1" } else { "" }
        );
        let (ws, _) = tungstenite::client(url.as_str(), tcp).expect("the WebSocket upgrade");
        // The upgrade is done: a thread of its own services the socket from here on.
        ws.get_ref().set_read_timeout(Some(TICK)).unwrap();
        let (outbound, to_send) = mpsc::channel();
        let (read, inbound) = mpsc::channel();
        std::thread::Builder::new()
            .name("dev-reload-client".to_owned())
            .spawn(move || pump(ws, &to_send, &read))
            .expect("the connection's thread starts");
        let mut client = Client {
            outbound,
            inbound,
            patience: PATIENCE,
            schema,
            seq: 0,
            next_call: 0,
            notices: Vec::new(),
            server_schema: 0,
            values: std::collections::HashMap::new(),
            closed: None,
        };
        let mut hello = Writer::new();
        Hello {
            undra_version: "0.1.0",
            schema_hash: schema,
            platform: "test",
            mode: "dev",
        }
        .encode(&mut hello);
        client.send(Kind::Hello, hello.as_slice());
        match client.read() {
            Got::Frame(Kind::Hello, payload) => {
                let hello = Hello::decode(&mut Reader::new(&payload)).unwrap();
                client.server_schema = hello.schema_hash;
            }
            other => panic!(
                "the server answers a Hello with its own, got {}",
                describe(&other)
            ),
        }
        client
    }

    fn send(&mut self, kind: Kind, payload: &[u8]) {
        let mut w = Writer::new();
        Envelope::write(&mut w, kind, self.seq, self.schema, payload);
        self.seq += 1;
        // A connection that has ended has no thread to take it; the next read says so.
        let _ = self.outbound.send(w.into_vec());
    }

    /// The next envelope, a close or silence; Log records addressed to the developer are noted.
    fn read(&mut self) -> Got {
        loop {
            let message = match self.inbound.recv_timeout(self.patience) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => return Got::Silence,
                // Nothing more will come: the connection ended (after its Close, if it had one).
                Err(RecvTimeoutError::Disconnected) => return Got::Closed(None),
            };
            match message {
                Ok(Message::Binary(bytes)) => {
                    let envelope = Envelope::parse(&bytes).unwrap();
                    if envelope.kind == Kind::ChangeSet {
                        if let Ok(change_set) =
                            ChangeSet::decode(&mut Reader::new(envelope.payload))
                        {
                            for entry in change_set.entries {
                                self.values
                                    .insert((entry.handle.0, entry.signal_id), entry.value);
                            }
                        }
                    }
                    if envelope.kind == Kind::Log {
                        if let Ok(log) = Log::decode(&mut Reader::new(envelope.payload)) {
                            if log.target == "undra::dev" {
                                self.notices.push(log.message.to_owned());
                            }
                        }
                    }
                    return Got::Frame(envelope.kind, envelope.payload.to_vec());
                }
                Ok(Message::Close(frame)) => {
                    return Got::Closed(frame.map(|f| (u16::from(f.code), f.reason.into_owned())));
                }
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                Ok(Message::Text(_)) => panic!("a text message"),
                Err(_) => return Got::Closed(None),
            }
        }
    }

    /// [`read`](Client::read) with `wait` as the patience, for this read only.
    fn read_within(&mut self, wait: Duration) -> Got {
        let usual = std::mem::replace(&mut self.patience, wait);
        let got = self.read();
        self.patience = usual;
        got
    }

    /// Reads until the server closes; returns the close code and reason.
    fn expect_close(&mut self) -> (u16, String) {
        loop {
            match self.read() {
                Got::Frame(..) => {}
                Got::Closed(close) => return close.expect("a Close frame"),
                Got::Silence => panic!("the server did not close"),
            }
        }
    }

    /// Calls and returns the reply's status and body.
    fn call(&mut self, target: CallTarget, args: &[u8]) -> (ReplyStatus, Vec<u8>) {
        self.next_call += 1;
        let id = self.next_call;
        let mut call = Writer::new();
        Call {
            target,
            call_id: id,
            args,
        }
        .encode(&mut call);
        self.send(Kind::Call, call.as_slice());
        loop {
            match self.read() {
                Got::Frame(Kind::Reply, payload) => {
                    let reply = Reply::decode(&mut Reader::new(&payload)).unwrap();
                    if reply.call_id == id {
                        return (reply.status, reply.body.to_vec());
                    }
                }
                Got::Frame(..) => {}
                Got::Closed(close) => panic!("closed while waiting for a reply: {close:?}"),
                Got::Silence => panic!("no reply"),
            }
        }
    }

    /// Sends a call without waiting for its reply; returns its id.
    fn send_call(&mut self, target: CallTarget, args: &[u8]) -> u32 {
        self.next_call += 1;
        let id = self.next_call;
        let mut call = Writer::new();
        Call {
            target,
            call_id: id,
            args,
        }
        .encode(&mut call);
        self.send(Kind::Call, call.as_slice());
        id
    }

    /// The reply to call `id`, or `None` when the server closes first (the close is consumed).
    fn reply_or_close(&mut self, id: u32) -> Option<(ReplyStatus, Vec<u8>)> {
        loop {
            match self.read() {
                Got::Frame(Kind::Reply, payload) => {
                    let reply = Reply::decode(&mut Reader::new(&payload)).unwrap();
                    if reply.call_id == id {
                        return Some((reply.status, reply.body.to_vec()));
                    }
                }
                Got::Frame(..) => {}
                Got::Closed(close) => {
                    self.closed = close;
                    return None;
                }
                Got::Silence => panic!("neither a reply nor a close"),
            }
        }
    }

    fn construct(&mut self, type_name: &str) -> u64 {
        let (status, body) = self.call(
            CallTarget::Constructor {
                type_id: ids::type_id(type_name),
                method_id: ids::method_id(type_name, "new"),
            },
            &[],
        );
        assert_eq!(status, ReplyStatus::Ok, "{type_name}::new");
        u64::decode_exact(&body).unwrap()
    }

    fn method(
        &mut self,
        handle: u64,
        type_name: &str,
        name: &str,
        args: &[u8],
    ) -> (ReplyStatus, Vec<u8>) {
        self.call(
            CallTarget::Method {
                handle: Handle(handle),
                method_id: ids::method_id(type_name, name),
            },
            args,
        )
    }

    /// Observes every signal of `handle` and returns the first change-set that carries entries
    /// for it, as `signal_id -> value bytes`.
    fn observe(&mut self, handle: u64) -> std::collections::HashMap<u32, Vec<u8>> {
        let mut w = Writer::new();
        Observe {
            handle: Handle(handle),
            signal_id: u32::MAX,
            on: true,
        }
        .encode(&mut w);
        self.send(Kind::Observe, w.as_slice());
        loop {
            match self.read() {
                Got::Frame(Kind::ChangeSet, payload) => {
                    let change_set = ChangeSet::decode(&mut Reader::new(&payload)).unwrap();
                    let mine: std::collections::HashMap<u32, Vec<u8>> = change_set
                        .entries
                        .iter()
                        .filter(|e| e.handle.0 == handle)
                        .map(|e| (e.signal_id, e.value.clone()))
                        .collect();
                    if !mine.is_empty() {
                        return mine;
                    }
                }
                Got::Frame(..) => {}
                Got::Closed(close) => panic!("closed while observing: {close:?}"),
                Got::Silence => panic!("no change-set after Observe"),
            }
        }
    }

    /// Reads until the count of `handle` is `expected` (a change-set that follows a command).
    fn await_count(&mut self, handle: u64, expected: i32) {
        for _ in 0..50 {
            if self
                .values
                .get(&(handle, COUNT))
                .is_some_and(|v| i32_of(v) == expected)
            {
                return;
            }
            let _ = self.read_within(Duration::from_millis(200));
        }
        panic!(
            "the count never became {expected}: {:?}",
            self.values.get(&(handle, COUNT))
        );
    }

    /// The `undra::dev` sentences that arrive within a short while.
    fn notices_after(&mut self, wait: Duration) -> Vec<String> {
        while let Got::Frame(..) = self.read_within(wait) {}
        self.notices.clone()
    }

    /// The notices once the first has arrived (the server sends it after the reconnect, however long a loaded
    /// machine takes), and what follows it closely.
    fn notices_when_told(&mut self) -> Vec<String> {
        while self.notices.is_empty() {
            if !matches!(self.read(), Got::Frame(..)) {
                break;
            }
        }
        self.notices_after(Duration::from_millis(500))
    }
}

fn describe(got: &Got) -> String {
    match got {
        Got::Frame(kind, _) => format!("a {kind:?} frame"),
        Got::Closed(close) => format!("a close {close:?}"),
        Got::Silence => "silence".to_owned(),
    }
}

fn i32_of(bytes: &[u8]) -> i32 {
    i32::decode_exact(bytes).unwrap()
}

fn u32_of(bytes: &[u8]) -> u32 {
    u32::decode_exact(bytes).unwrap()
}

/// Appends `text` to `file` (an edit the watcher sees).
fn append(file: &Path, text: &str) {
    let mut f = std::fs::OpenOptions::new().append(true).open(file).unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

/// The Counter's signals, in declaration order, and the BigList's.
const COUNT: u32 = 0;
const CHANGES: u32 = 1;
const ITEMS_COUNT: u32 = 1;

#[test]
fn a_rebuild_keeps_the_screen_the_client_was_on() {
    let project = playground_copy("reload-keep");
    let dev = Dev::start(&project, &[]);
    let counter_rs = project.root.join("core/src/counter.rs");

    // The app: a counter at 7, a 10,000-row list with its first row removed, and a plain object.
    let mut client = Client::connect(&dev, dev.hash, "reload-keep-token", false);
    let counter = client.construct("Counter");
    client.method(counter, "Counter", "add", &5_i32.encode_to_vec());
    client.method(counter, "Counter", "add", &2_i32.encode_to_vec());
    let list = client.construct("BigList");
    let (status, _) = client.method(list, "BigList", "remove_at", &0_u32.encode_to_vec());
    assert_eq!(status, ReplyStatus::Ok);
    let probe = client.construct("Probe");
    let values = client.observe(counter);
    assert_eq!((i32_of(&values[&COUNT]), u32_of(&values[&CHANGES])), (7, 2));
    assert_eq!(u32_of(&client.observe(list)[&ITEMS_COUNT]), 9_999);

    // A broken edit: the rebuild fails, the old core keeps serving and keeps its state.
    append(&counter_rs, "\npub fn broken( {\n");
    dev.wait_log(
        "the rebuild failed; still serving the previous build",
        BUILD,
    );
    let (status, _) = client.method(counter, "Counter", "add", &0_i32.encode_to_vec());
    assert_eq!(status, ReplyStatus::Ok, "the old core still answers");

    // A good edit: rebuilt, swapped, and the state carried over.
    std::fs::write(
        &counter_rs,
        std::fs::read_to_string(&counter_rs)
            .unwrap()
            .replace("\npub fn broken( {\n", "\n// touched\n"),
    )
    .unwrap();
    let restarted = dev.wait_line("Restarted: ws://", BUILD);
    // `cargo test -- --nocapture` shows what a developer reads in the terminal.
    eprintln!("{restarted}");
    assert!(restarted.contains("state kept (2 stores,"), "{restarted}");
    assert!(
        restarted.contains("1 object not carried over"),
        "{restarted}"
    );

    // The old connection was closed for the reload.
    let (code, reason) = client.expect_close();
    assert_eq!(code, 1001);
    assert!(reason.contains("reloading"), "{reason}");

    // The client comes back the way a platform runtime does: same token, resume, observe.
    let mut back = Client::connect(&dev, dev.hash, "reload-keep-token", true);
    let values = back.observe(counter);
    assert_eq!(
        (i32_of(&values[&COUNT]), u32_of(&values[&CHANGES])),
        (7, 3),
        "the counter has the value (and the change tally: the probe `add(0)` counted) it had before the rebuild"
    );
    assert_eq!(
        u32_of(&back.observe(list)[&ITEMS_COUNT]),
        9_999,
        "so does the 10,000-row list"
    );
    // Its handles are live: a command works on the restored store.
    back.method(counter, "Counter", "increment", &[]);
    back.await_count(counter, 8);
    // The plain object did not survive: the existing status 5 path.
    let (status, _) = back.method(probe, "Probe", "counters", &[]);
    assert_eq!(status, ReplyStatus::BadRequest, "a stale handle is refused");
    // The generation floor crossed the process boundary (ADR-022): the new process issues nothing
    // at or below the old one's generations, so the stale handle can never name a new object.
    let fresh_probe = back.construct("Probe");
    assert_ne!(fresh_probe, probe);
    assert!(
        Handle(fresh_probe).generation() > Handle(probe).generation(),
        "a handle made after the reload ({:?}) is above every handle of the old process ({:?})",
        Handle(fresh_probe),
        Handle(probe)
    );
    let (status, _) = back.method(probe, "Probe", "counters", &[]);
    assert_eq!(status, ReplyStatus::BadRequest, "still stale");

    assert_eq!(
        back.notices_after(Duration::from_millis(500)),
        ["Reloaded, state kept (1 object not carried over)"]
    );
    drop(back);
    dev.kill_and_expect_the_port_to_close();
}

#[test]
fn an_additive_schema_change_keeps_the_state() {
    // ADR-037: a restore matches signals by name and migrates what changed structurally, so a
    // schema change (here: a method added) no longer means fresh state.
    let project = playground_copy("reload-schema");
    let dev = Dev::start(&project, &[]);
    let counter_rs = project.root.join("core/src/counter.rs");

    let mut client = Client::connect(&dev, dev.hash, "reload-schema-token", false);
    let counter = client.construct("Counter");
    client.method(counter, "Counter", "add", &5_i32.encode_to_vec());
    assert_eq!(i32_of(&client.observe(counter)[&COUNT]), 5);

    // A new method is a new schema hash.
    let original = std::fs::read_to_string(&counter_rs).unwrap();
    let edited = original.replace(
        "    /// Adds one.\n    pub fn increment",
        "    /// A method the schema did not have.\n    pub fn added_by_the_test(&self) -> i32 {\n        7\n    }\n\n    /// Adds one.\n    pub fn increment",
    );
    assert_ne!(original, edited, "the edit applies");
    std::fs::write(&counter_rs, edited).unwrap();
    let restarted = dev.wait_restart_changing_schema(dev.hash, BUILD);
    eprintln!("{restarted}");
    assert!(restarted.contains("state kept (1 store,"), "{restarted}");
    let new_hash = restarted
        .split("schema hash ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .map(|hash| u64::from_str_radix(hash.trim_start_matches("0x"), 16).unwrap())
        .expect("the new hash is printed");
    assert_ne!(new_hash, dev.hash);

    // An app built from the old bindings is told so by the server's Hello, then closed (1008): R7.
    let (code, _) = client.expect_close();
    assert_eq!(code, 1001);
    let mut stale = Client::connect(&dev, dev.hash, "reload-schema-token", true);
    assert_eq!(
        stale.server_schema, new_hash,
        "the Hello carries the new core's hash"
    );
    assert_eq!(stale.expect_close().0, 1008);

    // The app on the new bindings finds its counter where it was, with the value it had.
    let mut back = Client::connect(&dev, new_hash, "reload-schema-token", true);
    let values = back.observe(counter);
    assert_eq!(
        (i32_of(&values[&COUNT]), u32_of(&values[&CHANGES])),
        (5, 1),
        "the counter survived the schema change"
    );
    back.method(counter, "Counter", "increment", &[]);
    back.await_count(counter, 6);
    let notices = back.notices_after(Duration::from_millis(500));
    assert_eq!(
        notices,
        ["Reloaded, state kept (the schema changed)"],
        "{notices:?}"
    );
    drop(back);
    dev.kill_and_expect_the_port_to_close();
}

#[test]
fn a_schema_change_the_state_cannot_follow_resets_it_and_says_why() {
    // `changes` renamed to `edits` (no `#[undra(default)]`, no hook): not structural, so the new
    // core refuses the snapshot as a whole and starts fresh, naming the store and the signal.
    let project = playground_copy("reload-schema-refused");
    let dev = Dev::start(&project, &[]);
    let counter_rs = project.root.join("core/src/counter.rs");

    let mut client = Client::connect(&dev, dev.hash, "reload-refused-token", false);
    let counter = client.construct("Counter");
    client.method(counter, "Counter", "add", &5_i32.encode_to_vec());
    assert_eq!(i32_of(&client.observe(counter)[&COUNT]), 5);

    let original = std::fs::read_to_string(&counter_rs).unwrap();
    let edited = original.replace("changes", "edits");
    assert_ne!(original, edited, "the edit applies");
    std::fs::write(&counter_rs, edited).unwrap();
    let restarted = dev.wait_restart_changing_schema(dev.hash, BUILD);
    eprintln!("{restarted}");
    assert!(
        restarted.contains("state reset: the core refused the snapshot")
            && restarted.contains("Counter")
            && restarted.contains("edits"),
        "{restarted}"
    );
    let new_hash = restarted
        .split("schema hash ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .map(|hash| u64::from_str_radix(hash.trim_start_matches("0x"), 16).unwrap())
        .expect("the new hash is printed");
    let (code, _) = client.expect_close();
    assert_eq!(code, 1001);

    // The rebuilt app starts on fresh state, and is told why.
    let mut fresh = Client::connect(&dev, new_hash, "reload-refused-fresh", false);
    let counter = fresh.construct("Counter");
    assert_eq!(i32_of(&fresh.observe(counter)[&COUNT]), 0, "fresh values");
    let notices = fresh.notices_after(Duration::from_millis(500));
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].starts_with("Reloaded, state reset: the core refused the snapshot"),
        "{notices:?}"
    );
    drop(fresh);
    dev.kill_and_expect_the_port_to_close();
}

#[test]
fn no_keep_state_starts_every_rebuilt_core_fresh() {
    let project = playground_copy("reload-fresh");
    let dev = Dev::start(&project, &["--no-keep-state"]);
    let counter_rs = project.root.join("core/src/counter.rs");

    let mut client = Client::connect(&dev, dev.hash, "reload-fresh-token", false);
    let counter = client.construct("Counter");
    client.method(counter, "Counter", "add", &5_i32.encode_to_vec());
    assert_eq!(i32_of(&client.observe(counter)[&COUNT]), 5);

    append(&counter_rs, "\n// touched\n");
    let restarted = dev.wait_line("Restarted: ws://", BUILD);
    assert!(
        restarted.contains("state reset: undra dev --no-keep-state"),
        "{restarted}"
    );
    assert_eq!(client.expect_close().0, 1001);

    // Its session is not carried: asking to resume it is "session lost" (4001), today's path.
    let mut back = Client::connect(&dev, dev.hash, "reload-fresh-token", true);
    assert_eq!(back.expect_close().0, 4001);
    dev.kill_and_expect_the_port_to_close();
}

#[test]
fn a_state_over_the_limit_falls_back_to_fresh_state_and_says_so() {
    // The limit is 16 MiB; the integration test lowers it so that the playground's 10,000-row list
    // (about 200 KB of snapshot) is over it.
    let project = playground_copy("reload-big");
    let mut cmd = project.undra();
    cmd.env("UNDRA_DEV_STATE_LIMIT_BYTES", "50000");
    let dev = Dev::start_command(cmd, &[]);
    let counter_rs = project.root.join("core/src/counter.rs");

    let mut client = Client::connect(&dev, dev.hash, "reload-big-token", false);
    let list = client.construct("BigList");
    assert_eq!(u32_of(&client.observe(list)[&ITEMS_COUNT]), 10_000);

    append(&counter_rs, "\n// touched\n");
    let restarted = dev.wait_line("Restarted: ws://", BUILD);
    assert!(
        restarted.contains("state reset: snapshot over 50000 bytes"),
        "{restarted}"
    );
    assert_eq!(client.expect_close().0, 1001);

    // Its session is not carried (4001), and the app that loads afresh is told why.
    let mut back = Client::connect(&dev, dev.hash, "reload-big-token", true);
    assert_eq!(back.expect_close().0, 4001);
    let mut fresh = Client::connect(&dev, dev.hash, "reload-big-fresh", false);
    let notices = fresh.notices_after(Duration::from_millis(500));
    assert_eq!(
        notices,
        ["Reloaded, state reset: snapshot over 50000 bytes"]
    );
    drop(fresh);
    dev.kill_and_expect_the_port_to_close();
}

// ----- review (2026-10-02): the attacks of the adversarial review ---------------------------------

/// The counter's value and change tally as the server last sent them.
fn count_of(client: &Client, handle: u64) -> Option<(i32, u32)> {
    Some((
        i32_of(client.values.get(&(handle, COUNT))?),
        u32_of(client.values.get(&(handle, CHANGES))?),
    ))
}

#[test]
fn calls_the_reload_cut_off_are_counted_and_the_notice_says_so() {
    // A write the client made during the reload must not vanish behind "Reloaded, state kept": one
    // call is still running when the old core is suspended (it never finishes: it is cancelled at the
    // end of the settle) and one is sent while the server no longer runs calls (it is never run).
    let project = playground_copy("reload-calls");
    let dev = Dev::start(&project, &[]);
    let counter_rs = project.root.join("core/src/counter.rs");

    let mut client = Client::connect(&dev, dev.hash, "reload-calls-token", false);
    let counter = client.construct("Counter");
    client.method(counter, "Counter", "add", &3_i32.encode_to_vec());
    let probe = client.construct("Probe");
    let hang = client.send_call(
        CallTarget::Method {
            handle: Handle(probe),
            method_id: ids::method_id("Probe", "hang"),
        },
        &[],
    );

    append(&counter_rs, "\n// touched\n");
    // The app keeps tapping "+1" through the rebuild and the swap: every tap that is answered landed;
    // the first one that is not was made after the server stopped running calls.
    let mut landed = 0;
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let id = client.send_call(
            CallTarget::Method {
                handle: Handle(counter),
                method_id: ids::method_id("Counter", "add"),
            },
            &1_i32.encode_to_vec(),
        );
        match client.reply_or_close(id) {
            Some((ReplyStatus::Ok, _)) => landed += 1,
            Some(other) => panic!("an add failed: {other:?}"),
            None => break,
        }
    }
    assert_eq!(
        client.closed.as_ref().map(|(code, _)| *code),
        Some(1001),
        "{:?}",
        client.closed
    );
    let _ = hang;

    let restarted = dev.wait_line("Restarted: ws://", BUILD);
    eprintln!("{restarted}");
    assert!(restarted.contains("state kept (1 store,"), "{restarted}");
    assert!(
        restarted.contains("1 call still running when the core was replaced was cancelled"),
        "{restarted}"
    );
    assert!(
        restarted.contains("1 call sent during the reload was not run"),
        "{restarted}"
    );

    let mut back = Client::connect(&dev, dev.hash, "reload-calls-token", true);
    back.observe(counter);
    assert_eq!(
        count_of(&back, counter),
        Some((3 + landed, 1 + landed as u32)),
        "every answered add is in the state, the unanswered one is not"
    );
    assert_eq!(
        back.notices_after(Duration::from_millis(500)),
        ["Reloaded, state kept (1 object not carried over; 2 calls lost in the reload)"]
    );
    drop(back);
    dev.kill_and_expect_the_port_to_close();
}

#[test]
fn a_rebuilt_core_that_does_not_start_leaves_the_old_one_serving_with_its_state() {
    // A real core that cannot start (an init hook that exits the process, as an abort in start-up code
    // would): the old core keeps serving, the client is never disconnected, nothing is lost; the next
    // good edit swaps with the state.
    let project = playground_copy("reload-nostart");
    let dev = Dev::start(&project, &[]);
    let counter_rs = project.root.join("core/src/counter.rs");
    let original = std::fs::read_to_string(&counter_rs).unwrap();

    let mut client = Client::connect(&dev, dev.hash, "reload-nostart-token", false);
    let counter = client.construct("Counter");
    client.method(counter, "Counter", "add", &6_i32.encode_to_vec());
    client.observe(counter);

    let exits = "\nundra::runtime::inventory::submit! {\n    undra::runtime::InitHook { name: \"review.exits-at-start\", run: |_| std::process::exit(3) }\n}\n";
    append(&counter_rs, exits);
    dev.wait_log(
        "the rebuilt core did not start; still serving the previous build",
        BUILD,
    );
    // Same socket, same core, same state: the client never noticed.
    let (status, _) = client.method(counter, "Counter", "add", &1_i32.encode_to_vec());
    assert_eq!(status, ReplyStatus::Ok);
    client.await_count(counter, 7);

    std::fs::write(&counter_rs, format!("{original}\n// fixed\n")).unwrap();
    let restarted = dev.wait_line("Restarted: ws://", BUILD);
    assert!(restarted.contains("state kept"), "{restarted}");
    assert_eq!(client.expect_close().0, 1001);
    let mut back = Client::connect(&dev, dev.hash, "reload-nostart-token", true);
    back.observe(counter);
    assert_eq!(count_of(&back, counter), Some((7, 2)));
    drop(back);
    dev.kill_and_expect_the_port_to_close();
}

#[test]
fn a_snapshot_the_new_core_refuses_falls_back_to_fresh_state_and_says_why() {
    // The rebuilt core has the same schema but its store cannot be restored (its restore hook
    // panics): `Runtime::restore` is all-or-nothing, the runner starts fresh and says why.
    let project = playground_copy("reload-refused");
    let dev = Dev::start(&project, &[]);
    let counter_rs = project.root.join("core/src/counter.rs");

    let mut client = Client::connect(&dev, dev.hash, "reload-refused-token", false);
    let counter = client.construct("Counter");
    client.method(counter, "Counter", "add", &5_i32.encode_to_vec());
    client.observe(counter);

    let original = std::fs::read_to_string(&counter_rs).unwrap();
    let edited = original.replace(
        "#[undra::store(restore = \"Self::assemble\")]",
        "#[undra::store(restore = \"Self::refuse\")]",
    ) + "\nimpl Counter {\n    fn refuse(_ctx: Ctx, _count: Signal<i32>, _changes: Signal<u32>) -> Self {\n        panic!(\"this build refuses every snapshot\")\n    }\n}\n";
    assert_ne!(original, edited);
    std::fs::write(&counter_rs, edited).unwrap();
    let restarted = dev.wait_line("Restarted: ws://", BUILD);
    eprintln!("{restarted}");
    assert!(
        restarted.contains("state reset: the core refused the snapshot"),
        "{restarted}"
    );
    let new_hash = restarted
        .split("schema hash ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .map(|hash| u64::from_str_radix(hash.trim_start_matches("0x"), 16).unwrap())
        .expect("the new hash is printed");
    assert_eq!(
        new_hash, dev.hash,
        "a restore hook is not part of the schema"
    );
    assert_eq!(client.expect_close().0, 1001);

    // Today's fallback: the session is not carried (4001), the app loads afresh and is told why.
    let mut back = Client::connect(&dev, dev.hash, "reload-refused-token", true);
    assert_eq!(back.expect_close().0, 4001);
    let mut fresh = Client::connect(&dev, dev.hash, "reload-refused-fresh", false);
    let counter = fresh.construct("Counter");
    fresh.observe(counter);
    assert_eq!(count_of(&fresh, counter), Some((0, 0)));
    let notices = fresh.notices_after(Duration::from_millis(500));
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].starts_with("Reloaded, state reset: the core refused the snapshot"),
        "{notices:?}"
    );
    drop(fresh);
    dev.kill_and_expect_the_port_to_close();
}

#[test]
fn a_change_during_a_reload_is_built_next_and_the_state_survives_both_swaps() {
    // A second save while the first rebuild and swap run: no double swap, no lost runner; the change
    // is built after the swap, and the state is carried twice (the second time from a core whose
    // client has not come back yet: the inherited session is handed on).
    let project = playground_copy("reload-twice");
    let dev = Dev::start(&project, &[]);
    let counter_rs = project.root.join("core/src/counter.rs");

    let mut client = Client::connect(&dev, dev.hash, "reload-twice-token", false);
    let counter = client.construct("Counter");
    client.method(counter, "Counter", "add", &4_i32.encode_to_vec());
    client.observe(counter);

    append(&counter_rs, "\n// first\n");
    dev.wait_log("Change detected, rebuilding", BUILD);
    append(&counter_rs, "\n// second\n");
    let first = dev.wait_line("Restarted: ws://", BUILD);
    let second = dev.wait_line("Restarted: ws://", BUILD);
    eprintln!("{first}\n{second}");
    assert!(first.contains("state kept (1 store,"), "{first}");
    assert!(second.contains("state kept (1 store,"), "{second}");
    assert_eq!(client.expect_close().0, 1001);
    assert_eq!(
        dev.log
            .lock()
            .unwrap()
            .matches("Change detected, rebuilding")
            .count(),
        2,
        "two rebuilds, one per change that was not yet built"
    );

    let mut back = Client::connect(&dev, dev.hash, "reload-twice-token", true);
    back.observe(counter);
    assert_eq!(count_of(&back, counter), Some((4, 1)));
    drop(back);
    dev.kill_and_expect_the_port_to_close();
}

// ----- query handles and lazy lists across a rebuild (ADR-059) -----------------------------------------

/// `new TickerQueryHandle()`: the playground's polling query, the Remote tab's kind of handle.
fn ticker_handle(client: &mut Client) -> u64 {
    let ticker = ids::fnv1a32("query.ticker");
    let (status, body) = client.call(
        CallTarget::Constructor {
            type_id: ticker,
            method_id: ticker,
        },
        &[],
    );
    assert_eq!(status, ReplyStatus::Ok, "TickerQueryHandle()");
    u64::decode_exact(&body).unwrap()
}

/// The ticker's `data` (signal 0) as the client last saw it.
fn ticks(client: &Client, handle: u64) -> Option<u32> {
    client
        .values
        .get(&(handle, 0))
        .and_then(|v| Option::<u32>::decode_exact(v).ok().flatten())
}

/// Reads until the ticker has shown `at_least`.
fn await_ticks(client: &mut Client, handle: u64, at_least: u32) {
    for _ in 0..300 {
        if ticks(client, handle).is_some_and(|n| n >= at_least) {
            return;
        }
        let _ = client.read_within(Duration::from_millis(200));
    }
    panic!(
        "the ticker never reached {at_least}: {:?}",
        ticks(client, handle)
    );
}

/// A page of the library's `books` through the page server its `LazyValue` (signal 0) names: the
/// list's total and how many rows the page held.
fn page_of_books(client: &mut Client, library: u64) -> (u32, u32) {
    let value = client
        .values
        .get(&(library, 0))
        .cloned()
        .expect("the library's books were observed");
    let lazy = LazyValue::decode(&mut Reader::new(&value)).expect("a LazyValue");
    let (status, body) = client.call(
        CallTarget::LazyPage {
            handle: lazy.handle,
            offset: 0,
            limit: 3,
        },
        &[],
    );
    assert_eq!(status, ReplyStatus::Ok, "a page of the books");
    let page = LazyPage::decode(&mut Reader::new(&body)).expect("a page header");
    (page.total, page.count)
}

/// The Remote tab across a rebuild: a raw client holding the polling `ticker` query's handle and a
/// `Library`, whose `books` it pages. The edit rebuilds the core; the client reconnects and observes
/// again, as every platform runtime does, and runs no code of its own for the handle.
#[test]
fn a_query_handle_and_a_paged_list_keep_working_across_a_rebuild() {
    let project = playground_copy("reload-query");
    let dev = Dev::start(&project, &[]);
    let paging_rs = project.root.join("core/src/paging.rs");
    let refetch = ids::fnv1a32("query.refetch");

    let mut client = Client::connect(&dev, dev.hash, "reload-query-token", false);
    let counter = client.construct("Counter");
    client.method(counter, "Counter", "add", &5_i32.encode_to_vec());
    let ticker = ticker_handle(&mut client);
    let library = client.construct("Library");
    client.observe(ticker);
    client.observe(library);
    await_ticks(&mut client, ticker, 2);
    assert_eq!(page_of_books(&mut client, library), (10_000, 3));

    append(&paging_rs, "\n// touched\n");
    let restarted = dev.wait_line("Restarted: ws://", BUILD);
    eprintln!("{restarted}");
    assert!(
        restarted.contains("state kept (2 stores, 1 query handle,"),
        "{restarted}"
    );
    assert!(!restarted.contains("not carried over"), "{restarted}");
    let (code, _) = client.expect_close();
    assert_eq!(code, 1001);

    let mut back = Client::connect(&dev, dev.hash, "reload-query-token", true);
    // The same handle value answers the observe with the handle's values (a fresh core: nothing
    // cached, so it is fetching), and a `refetch` on it is accepted: status 5 before ADR-059.
    let first = back.observe(ticker);
    assert!(
        first.contains_key(&1),
        "status arrives for the same handle: {first:?}"
    );
    let (status, _) = back.call(
        CallTarget::Method {
            handle: Handle(ticker),
            method_id: refetch,
        },
        &[],
    );
    assert_eq!(
        status,
        ReplyStatus::Ok,
        "refetch on the handle the client kept"
    );
    // The new core fetches and keeps polling: the tick counter of the new process climbs from 1.
    let mut seen = Vec::new();
    for _ in 0..300 {
        if let Some(n) = ticks(&back, ticker) {
            if seen.last() != Some(&n) {
                seen.push(n);
            }
        }
        if seen.len() >= 3 {
            break;
        }
        let _ = back.read_within(Duration::from_millis(200));
    }
    eprintln!("ticks seen after the reload: {seen:?}");
    assert!(
        seen.len() >= 3,
        "polling continues after the reload: {seen:?}"
    );
    // The `Library`'s page server is a new one in the new core; the host's `Observe` names it.
    back.observe(library);
    assert_eq!(page_of_books(&mut back, library), (10_000, 3));
    let values = back.observe(counter);
    assert_eq!(i32_of(&values[&COUNT]), 5);
    assert_eq!(back.notices_when_told(), ["Reloaded, state kept"]);
    drop(back);
    dev.kill_and_expect_the_port_to_close();
}

/// An edit that changes the parameter type of a query: its handle is refused by the restore, counted
/// in the notice with the objects not carried over, and the rest of the state is kept.
#[test]
fn a_query_whose_parameter_type_the_edit_changes_is_not_carried_over_and_the_rest_is() {
    let project = playground_copy("reload-query-schema");
    let dev = Dev::start(&project, &[]);
    let updates_rs = project.root.join("core/src/updates.rs");
    let roster = ids::fnv1a32("query.roster");
    let refetch = ids::fnv1a32("query.refetch");

    let mut client = Client::connect(&dev, dev.hash, "reload-query-schema-token", false);
    let counter = client.construct("Counter");
    client.method(counter, "Counter", "add", &5_i32.encode_to_vec());
    let (status, body) = client.call(
        CallTarget::Constructor {
            type_id: roster,
            method_id: roster,
        },
        &7_u32.encode_to_vec(),
    );
    assert_eq!(status, ReplyStatus::Ok, "RosterQueryHandle(7)");
    let roster_handle = u64::decode_exact(&body).unwrap();
    assert_eq!(i32_of(&client.observe(counter)[&COUNT]), 5);
    client.observe(roster_handle);

    // `team` was a number; the edit makes it a string (the query's id does not change).
    let original = std::fs::read_to_string(&updates_rs).unwrap();
    let edited = original.replacen(
        "pub async fn roster(_ctx: &Ctx, team: u32)",
        "pub async fn roster(_ctx: &Ctx, team: String)",
        1,
    );
    assert_ne!(original, edited, "the edit applies");
    std::fs::write(&updates_rs, edited).unwrap();
    // The restart that changes the schema: a loaded machine can rebuild once before the write lands.
    let restarted = dev.wait_restart_changing_schema(dev.hash, BUILD);
    eprintln!("{restarted}");
    assert!(restarted.contains("state kept (1 store,"), "{restarted}");
    assert!(
        restarted.contains("1 object not carried over"),
        "the handle of the query whose parameter changed is counted: {restarted}"
    );
    let new_hash = restarted
        .split("schema hash ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .map(|hash| u64::from_str_radix(hash.trim_start_matches("0x"), 16).unwrap())
        .expect("the new hash is printed");
    assert_ne!(new_hash, dev.hash);
    let (code, _) = client.expect_close();
    assert_eq!(code, 1001);

    let mut back = Client::connect(&dev, new_hash, "reload-query-schema-token", true);
    assert_eq!(
        i32_of(&back.observe(counter)[&COUNT]),
        5,
        "the store was restored"
    );
    let (status, _) = back.call(
        CallTarget::Method {
            handle: Handle(roster_handle),
            method_id: refetch,
        },
        &[],
    );
    assert_eq!(
        status,
        ReplyStatus::BadRequest,
        "a refused handle is stale, as every object a restore does not carry"
    );
    assert_eq!(
        back.notices_when_told(),
        ["Reloaded, state kept (the schema changed; 1 object not carried over)"]
    );
    let log = dev.log.lock().unwrap().clone();
    assert!(
        log.contains("is not re-issued: the types it was made from changed"),
        "the core says why: {log}"
    );
    drop(back);
    dev.kill_and_expect_the_port_to_close();
}
