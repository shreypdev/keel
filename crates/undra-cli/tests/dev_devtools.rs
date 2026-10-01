//! The devtools of `undra dev` (ADR-054) on a copy of the playground: the page behind its token, the
//! devtools socket, a change driven through an app client that the page sees, and time travel back,
//! with the app's state converging through its own session.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use common::devserver::Dev;
use common::playground_copy;
use tungstenite::{Message, WebSocket};
use undra_meta::ids;
use undra_transport::devtools::proto::{Cause, ClientMsg, Delivery, ServerMsg};
use undra_wire::payload::{Call, CallTarget, ChangeSet, Hello, Log, Observe, Reply, ReplyStatus};
use undra_wire::{Decode, Encode, Envelope, Handle, Kind, Reader, Writer};

/// `GET target` on `addr`: the status line and the body.
fn http_get(addr: &str, target: &str) -> (String, String, String) {
    let mut tcp = TcpStream::connect(addr).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        tcp,
        "GET {target} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut text = Vec::new();
    tcp.read_to_end(&mut text).unwrap();
    let text = String::from_utf8_lossy(&text).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.lines().next().unwrap_or("").to_owned();
    (status, head.to_owned(), body.to_owned())
}

/// Every process's command line on this machine, as `ps` shows them to every user.
fn all_command_lines() -> String {
    let out = std::process::Command::new("ps")
        .args(["-ww", "-ax", "-o", "args="])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The page address `undra dev` printed: `http://127.0.0.1:PORT/devtools?token=...`.
fn page_url(dev: &Dev) -> String {
    let line = dev.wait_line("devtools", Duration::from_secs(60));
    line.split_whitespace()
        .find(|w| w.starts_with("http://"))
        .unwrap_or_else(|| panic!("no page address in `{line}`"))
        .to_owned()
}

/// An app client, as a platform runtime's `remote` transport is.
struct App {
    ws: WebSocket<TcpStream>,
    schema: u64,
    seq: u32,
    next_call: u32,
    values: std::collections::HashMap<(u64, u32), Vec<u8>>,
    notices: Vec<String>,
}

impl App {
    fn connect(dev: &Dev) -> App {
        App::connect_session(dev, false)
    }

    /// Connects as the same client again after a reload, asking for its session back.
    fn resume(dev: &Dev) -> App {
        App::connect_session(dev, true)
    }

    fn connect_session(dev: &Dev, resume: bool) -> App {
        let tcp = TcpStream::connect(dev.addr()).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let url = format!(
            "{}/?undra_session=devtools-test{}",
            dev.url.trim_end_matches('/'),
            if resume { "&undra_resume=1" } else { "" }
        );
        let (ws, _) = tungstenite::client(url.as_str(), tcp).expect("the WebSocket upgrade");
        let mut app = App {
            ws,
            schema: dev.hash,
            seq: 0,
            next_call: 0,
            values: Default::default(),
            notices: Vec::new(),
        };
        let mut hello = Writer::new();
        Hello {
            undra_version: "0.1.0",
            schema_hash: dev.hash,
            platform: "test",
            mode: "dev",
        }
        .encode(&mut hello);
        app.send(Kind::Hello, hello.as_slice());
        let (kind, _) = app.read();
        assert_eq!(kind, Kind::Hello);
        app
    }

    fn send(&mut self, kind: Kind, payload: &[u8]) {
        let mut w = Writer::new();
        Envelope::write(&mut w, kind, self.seq, self.schema, payload);
        self.seq += 1;
        self.ws.send(Message::Binary(w.into_vec())).unwrap();
    }

    fn read(&mut self) -> (Kind, Vec<u8>) {
        loop {
            match self.ws.read().expect("the app socket stays open") {
                Message::Binary(bytes) => {
                    let env = Envelope::parse(&bytes).unwrap();
                    match env.kind {
                        Kind::ChangeSet => {
                            for e in ChangeSet::decode(&mut Reader::new(env.payload))
                                .unwrap()
                                .entries
                            {
                                self.values.insert((e.handle.0, e.signal_id), e.value);
                            }
                        }
                        Kind::Log => {
                            if let Ok(log) = Log::decode(&mut Reader::new(env.payload)) {
                                if log.target == "undra::dev" {
                                    self.notices.push(log.message.to_owned());
                                }
                            }
                        }
                        _ => {}
                    }
                    return (env.kind, env.payload.to_vec());
                }
                Message::Close(_) => panic!("the dev server closed the app"),
                _ => {}
            }
        }
    }

    fn call(&mut self, target: CallTarget, args: &[u8]) -> (ReplyStatus, Vec<u8>) {
        self.next_call += 1;
        let id = self.next_call;
        let mut w = Writer::new();
        Call {
            target,
            call_id: id,
            args,
        }
        .encode(&mut w);
        self.send(Kind::Call, w.as_slice());
        loop {
            let (kind, payload) = self.read();
            if kind == Kind::Reply {
                let reply = Reply::decode(&mut Reader::new(&payload)).unwrap();
                if reply.call_id == id {
                    return (reply.status, reply.body.to_vec());
                }
            }
        }
    }

    fn counter(&mut self) -> u64 {
        let (status, body) = self.call(
            CallTarget::Constructor {
                type_id: ids::type_id("Counter"),
                method_id: ids::method_id("Counter", "new"),
            },
            &[],
        );
        assert_eq!(status, ReplyStatus::Ok);
        u64::decode_exact(&body).unwrap()
    }

    fn add(&mut self, counter: u64, n: i32) {
        let (status, _) = self.call(
            CallTarget::Method {
                handle: Handle(counter),
                method_id: ids::method_id("Counter", "add"),
            },
            &n.encode_to_vec(),
        );
        assert_eq!(status, ReplyStatus::Ok);
    }

    /// Observes everything of `handle` (the first change-set arrives before this returns).
    fn observe_all(&mut self, handle: u64) {
        let mut w = Writer::new();
        Observe {
            handle: Handle(handle),
            signal_id: u32::MAX,
            on: true,
        }
        .encode(&mut w);
        self.send(Kind::Observe, w.as_slice());
        while !self.values.keys().any(|(h, _)| *h == handle) {
            self.read();
        }
    }

    /// Reads until `done` holds of what has been received, within 30 seconds.
    fn read_until(&mut self, what: &str, done: impl Fn(&App) -> bool) {
        self.ws
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            if let Ok(Message::Binary(bytes)) = self.ws.read() {
                {
                    let env = Envelope::parse(&bytes).unwrap();
                    match env.kind {
                        Kind::ChangeSet => {
                            for e in ChangeSet::decode(&mut Reader::new(env.payload))
                                .unwrap()
                                .entries
                            {
                                self.values.insert((e.handle.0, e.signal_id), e.value);
                            }
                        }
                        Kind::Log => {
                            if let Ok(log) = Log::decode(&mut Reader::new(env.payload)) {
                                if log.target == "undra::dev" {
                                    self.notices.push(log.message.to_owned());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn count(&self, counter: u64) -> i32 {
        i32::decode_exact(&self.values[&(counter, 0)]).unwrap()
    }
}

/// A devtools page as a raw client.
struct Page {
    ws: WebSocket<TcpStream>,
    seen: Vec<ServerMsg>,
}

impl Page {
    fn connect(page_url: &str) -> Page {
        let (addr, token) = {
            let rest = page_url.trim_start_matches("http://");
            let (addr, query) = rest.split_once("/devtools?").unwrap();
            (addr.to_owned(), query.to_owned())
        };
        let tcp = TcpStream::connect(&addr).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let (ws, _) = tungstenite::client(format!("ws://{addr}/devtools/ws?{token}").as_str(), tcp)
            .expect("the devtools socket upgrades with the token");
        Page {
            ws,
            seen: Vec::new(),
        }
    }

    fn until<T>(&mut self, what: &str, mut pick: impl FnMut(&ServerMsg) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(Instant::now() < deadline, "no {what} within 30 s");
            match self.ws.read() {
                Ok(Message::Binary(bytes)) => {
                    let msg = ServerMsg::decode(&bytes).expect("the server speaks the protocol");
                    let found = pick(&msg);
                    self.seen.push(msg);
                    if let Some(found) = found {
                        return found;
                    }
                }
                Ok(Message::Close(_)) => panic!("the page was closed while waiting for {what}"),
                _ => {}
            }
        }
    }

    fn send(&mut self, msg: ClientMsg) {
        let mut w = Writer::new();
        msg.encode(&mut w);
        self.ws.send(Message::Binary(w.into_vec())).unwrap();
    }
}

#[test]
fn the_page_is_served_behind_its_token_and_time_travel_restores_the_app() {
    let project = playground_copy("devtools-travel");
    let dev = Dev::start(&project, &["--no-watch"]);
    let url = page_url(&dev);
    let addr = dev.addr();
    let token = url.split("token=").nth(1).unwrap().to_owned();
    assert_eq!(token.len(), 32, "a token of 128 bits: {url}");

    // The page: the real assets, with the token put in, and a 404 for anyone without it.
    let (status, head, body) = http_get(&addr, &format!("/devtools?token={token}"));
    assert_eq!(status, "HTTP/1.1 200 OK", "{head}");
    assert!(head.contains("text/html"), "{head}");
    assert!(body.contains(&format!("app.js?token={token}")), "{body}");
    let (status, _, js) = http_get(&addr, &format!("/devtools/app.js?token={token}"));
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert!(
        js.len() > 10_000,
        "the bundle is served whole: {} bytes",
        js.len()
    );
    // The same token with its last digit changed.
    let last = if token.ends_with('0') { '1' } else { '0' };
    let wrong = format!("{}{last}", &token[..31]);
    let nothing = http_get(&addr, "/devtools");
    for target in [
        format!("/devtools?token={wrong}"),
        "/devtools/app.js".to_owned(),
        format!("/devtools/nope?token={token}"),
    ] {
        let got = http_get(&addr, &target);
        assert_eq!(got.0, "HTTP/1.1 404 Not Found", "{target}");
        assert_eq!(
            (got.1, got.2),
            (nothing.1.clone(), nothing.2.clone()),
            "{target}: every refusal reads the same"
        );
    }

    // The token is in the page's address and the runner's environment, and nowhere `ps` shows it to
    // other users: not in any process's command line (the runner's included).
    let command_lines = all_command_lines();
    assert!(
        !command_lines.contains(&token),
        "the token is on a command line:\n{}",
        command_lines
            .lines()
            .filter(|l| l.contains(&token))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        command_lines.contains("undra-dev-runner") || command_lines.contains("--devtools"),
        "the probe saw the runner's command line"
    );

    // The app: a counter at 7.
    let mut app = App::connect(&dev);
    let counter = app.counter();
    app.add(counter, 5);
    app.add(counter, 2);
    app.observe_all(counter);
    assert_eq!(app.count(counter), 7);

    // The page attaches and is told what the core is, what stores it has and their values.
    let mut page = Page::connect(&url);
    let welcome = page.until("the welcome", |m| match m {
        ServerMsg::Welcome(w) => Some(w.clone()),
        _ => None,
    });
    assert_eq!(welcome.schema_hash, dev.hash);
    assert!(welcome.schema_json.contains("\"Counter\""));
    page.until("the counter among the stores", |m| match m {
        ServerMsg::Stores(s) => s.iter().find(|s| s.handle == counter).map(|_| ()),
        _ => None,
    });
    let first_step = page.until("the first step", |m| match m {
        ServerMsg::Step(s) => Some(s.step),
        _ => None,
    });

    // A change the app makes: the page sees it, labelled with the call, and a step records it.
    app.add(counter, 3);
    let (set, cause) = page.until("the commit", |m| match m {
        ServerMsg::ChangeSet {
            delivery: Delivery::Commit,
            cause,
            payload,
            ..
        } => Some((
            ChangeSet::decode(&mut Reader::new(payload)).unwrap(),
            *cause,
        )),
        _ => None,
    });
    assert_eq!(cause, Cause::Call(ids::method_id("Counter", "add")));
    let count = set
        .entries
        .iter()
        .find(|e| e.handle.0 == counter && e.signal_id == 0)
        .expect("the count changed");
    assert_eq!(i32::decode_exact(&count.value).unwrap(), 10);
    let second_step = page.until("a step after the change", |m| match m {
        ServerMsg::Step(s) if s.step > first_step => Some(s.step),
        _ => None,
    });
    app.read_until("the count 10", |a| a.count(counter) == 10);

    // Travel back to the step before the change: the core is restored, and the app converges.
    page.send(ClientMsg::Restore {
        request_id: 1,
        step: first_step,
    });
    let traveled = page.until("the answer", |m| match m {
        ServerMsg::Traveled(t) => Some(t.clone()),
        _ => None,
    });
    assert!(traveled.ok && traveled.step == first_step, "{traveled:?}");
    app.read_until("the count back at 7", |a| a.count(counter) == 7);
    app.read_until("the dev bar's notice", |a| {
        a.notices
            .iter()
            .any(|n| n == &format!("time travel: step {first_step}"))
    });
    // The restore is on the page's timeline and in its history (a step, sent before the answer),
    // labelled with where it went.
    assert!(
        page.seen.iter().any(|m| matches!(m, ServerMsg::Step(s) if s.restored_from == first_step && s.step > second_step)),
        "no step records the restore: {:?}",
        page.seen.iter().filter(|m| matches!(m, ServerMsg::Step(_))).collect::<Vec<_>>()
    );
    assert!(
        page.seen.iter().any(|m| matches!(m, ServerMsg::ChangeSet { delivery: Delivery::Commit, cause: Cause::Restore(s), .. } if *s == first_step)),
        "the restore's change-set is not labelled"
    );
    // And the core really is there: the next change builds on 7.
    app.add(counter, 1);
    app.read_until("the count 8", |a| a.count(counter) == 8);

    // Nothing `undra dev` or the runner printed afterwards carries the token: the banner's address is
    // the only place it is written.
    let printed: Vec<String> = dev.lines.try_iter().collect();
    assert!(
        printed.iter().all(|l| !l.contains(&token)),
        "stdout leaked the token: {printed:?}"
    );
    assert!(
        !dev.log.lock().unwrap().contains(&token),
        "stderr leaked the token:\n{}",
        dev.log.lock().unwrap()
    );
}

#[test]
fn with_devtools_off_the_endpoint_does_not_exist() {
    let project = playground_copy("devtools-off");
    let dev = Dev::start(&project, &["--no-watch", "--devtools", "off"]);
    let addr = dev.addr();
    // The banner says nothing about a page (and nothing here guesses a token).
    let (status, _, _) = http_get(&addr, "/devtools?token=0123456789abcdef0123456789abcdef");
    assert_eq!(status, "HTTP/1.1 404 Not Found");
    let tcp = TcpStream::connect(&addr).unwrap();
    let refused = tungstenite::client(
        format!("ws://{addr}/devtools/ws?token=0123456789abcdef0123456789abcdef").as_str(),
        tcp,
    );
    assert!(
        matches!(refused, Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(r))) if r.status() == 404)
    );
    // The app's own socket is untouched.
    let mut app = App::connect(&dev);
    let counter = app.counter();
    app.add(counter, 2);
    app.observe_all(counter);
    assert_eq!(app.count(counter), 2);
}

#[test]
fn a_rebuild_with_a_page_open_replaces_the_runner_and_the_page_finds_the_new_core() {
    let project = playground_copy("devtools-reload");
    let dev = Dev::start(&project, &[]);
    let url = page_url(&dev);
    let mut app = App::connect(&dev);
    let counter = app.counter();
    app.add(counter, 5);
    app.observe_all(counter);
    let mut page = Page::connect(&url);
    let before = page.until("the welcome", |m| match m {
        ServerMsg::Welcome(w) => Some(w.core_epoch),
        _ => None,
    });
    page.until("the first step", |m| {
        matches!(m, ServerMsg::Step(_)).then_some(())
    });

    // An edit: the dev server suspends the old core (closing the page like any client), carries the
    // state over and starts the new one on the same address.
    let counter_rs = project.root.join("core/src/counter.rs");
    let mut source = std::fs::read_to_string(&counter_rs).unwrap();
    source.push_str("\n// touched\n");
    std::fs::write(&counter_rs, source).unwrap();
    dev.wait_line("Restarted: ws://", Duration::from_secs(600));
    // The page's socket was closed for the reload; it connects again, with the same token, to the
    // new core: a new epoch, the carried state, and a history that starts again.
    let mut page = Page::connect(&url);
    let after = page.until("the new welcome", |m| match m {
        ServerMsg::Welcome(w) => Some(w.core_epoch),
        _ => None,
    });
    assert_ne!(before, after, "a new core process says so");
    page.until("the carried counter among the stores", |m| match m {
        ServerMsg::Stores(s) => s.iter().find(|s| s.handle == counter).map(|_| ()),
        _ => None,
    });
    let step = page.until("a first step", |m| match m {
        ServerMsg::Step(s) => Some(s.step),
        _ => None,
    });
    assert_eq!(step, 1);
    // And the app, which reconnects with its session, finds its counter and still drives it: the
    // page sees the change.
    let mut app = App::resume(&dev);
    app.observe_all(counter);
    assert_eq!(app.count(counter), 5);
    app.add(counter, 1);
    page.until("the commit", |m| {
        matches!(
            m,
            ServerMsg::ChangeSet {
                delivery: Delivery::Commit,
                ..
            }
        )
        .then_some(())
    });
}
