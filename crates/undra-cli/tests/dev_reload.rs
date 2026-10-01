//! `undra dev` carries the core's state across a rebuild (ADR-053): the real `undra dev` on a
//! copy of the playground, a raw WebSocket client that behaves the way a platform runtime's
//! `remote` transport does (a session token, resume, observe), edits of the core's sources, and
//! what the client finds afterwards.

mod common;

use std::io::Write;
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

use common::devserver::Dev;
use common::playground_copy;
use tungstenite::{Message, WebSocket};
use undra_meta::ids;
use undra_wire::payload::{Call, CallTarget, ChangeSet, Hello, Log, Observe, Reply, ReplyStatus};
use undra_wire::{Decode, Encode, Envelope, Handle, Kind, Reader, Writer};

const BUILD: Duration = Duration::from_secs(600);

/// What a read from the server produced.
enum Got {
    Frame(Kind, Vec<u8>),
    Closed(Option<(u16, String)>),
    Silence,
}

/// A client of `undra dev` speaking the envelope by hand, with a session token as the platform
/// runtimes send (ADR-051).
struct Client {
    ws: WebSocket<TcpStream>,
    schema: u64,
    seq: u32,
    next_call: u32,
    /// The `undra::dev` sentences received so far.
    notices: Vec<String>,
    /// The hash in the server's Hello.
    server_schema: u64,
    /// The latest value received for each `(store handle, signal id)`.
    values: std::collections::HashMap<(u64, u32), Vec<u8>>,
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
        let mut client = Client {
            ws,
            schema,
            seq: 0,
            next_call: 0,
            notices: Vec::new(),
            server_schema: 0,
            values: std::collections::HashMap::new(),
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
        self.ws.send(Message::Binary(w.into_vec())).unwrap();
    }

    /// The next envelope, a close or silence; Log records addressed to the developer are noted.
    fn read(&mut self) -> Got {
        loop {
            match self.ws.read() {
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
                    let _ = self.ws.flush();
                    return Got::Closed(frame.map(|f| (u16::from(f.code), f.reason.into_owned())));
                }
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                Ok(Message::Text(_)) => panic!("a text message"),
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Got::Silence;
                }
                Err(_) => return Got::Closed(None),
            }
        }
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
            let _ = self
                .ws
                .get_ref()
                .set_read_timeout(Some(Duration::from_millis(200)));
            let _ = self.read();
        }
        let _ = self
            .ws
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(30)));
        panic!(
            "the count never became {expected}: {:?}",
            self.values.get(&(handle, COUNT))
        );
    }

    /// The `undra::dev` sentences that arrive within a short while.
    fn notices_after(&mut self, wait: Duration) -> Vec<String> {
        let _ = self.ws.get_ref().set_read_timeout(Some(wait));
        while let Got::Frame(..) = self.read() {}
        let _ = self
            .ws
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(30)));
        self.notices.clone()
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

    assert_eq!(
        back.notices_after(Duration::from_millis(500)),
        ["Reloaded, state kept (1 object not carried over)"]
    );
    drop(back);
    dev.kill_and_expect_the_port_to_close();
}

#[test]
fn a_schema_change_resets_the_state_and_says_so() {
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
    let restarted = dev.wait_line("Restarted: ws://", BUILD);
    assert!(
        restarted.contains("state reset: schema changed (was 0x"),
        "{restarted}"
    );
    let new_hash = restarted
        .split("schema hash ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .map(|hash| u64::from_str_radix(hash.trim_start_matches("0x"), 16).unwrap())
        .expect("the new hash is printed");
    assert_ne!(new_hash, dev.hash);

    // An app built from the old bindings is told so by the server's Hello, then closed (1008).
    let (code, _) = client.expect_close();
    assert_eq!(code, 1001);
    let mut stale = Client::connect(&dev, dev.hash, "reload-schema-token", true);
    assert_eq!(
        stale.server_schema, new_hash,
        "the Hello carries the new core's hash"
    );
    assert_eq!(stale.expect_close().0, 1008);

    // The rebuilt app starts on fresh state, and is told why.
    let mut fresh = Client::connect(&dev, new_hash, "reload-schema-fresh", false);
    let counter = fresh.construct("Counter");
    let values = fresh.observe(counter);
    assert_eq!(
        (i32_of(&values[&COUNT]), u32_of(&values[&CHANGES])),
        (0, 0),
        "fresh values"
    );
    let notices = fresh.notices_after(Duration::from_millis(500));
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].starts_with("Reloaded, state reset: schema changed"),
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
