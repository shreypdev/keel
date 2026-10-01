//! The devtools endpoint of a dev server (ADR-054): the page and its token, the socket, what a page
//! sees (every change-set, port calls, steps), what an app client keeps seeing (only what it
//! observed), and time travel.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use common::*;
use tungstenite::{Message, WebSocket};
use undra::wire::payload::{ChangeOp, ChangeSet, Log, PortCall, PortStatus};
use undra::wire::{Decode, Kind, Reader};
use undra_transport::devtools::proto::{Cause, ClientMsg, Delivery, PortRecord, ServerMsg};
use undra_transport::{Asset, DevtoolsConfig, ServerConfig};

const TOKEN: &str = "t0k3nt0k3nt0k3nZZ";

static ASSETS: &[Asset] = &[
    Asset {
        path: "index.html",
        content_type: "text/html; charset=utf-8",
        bytes: b"<html>__UNDRA_DEVTOOLS_TOKEN__</html>",
    },
    Asset {
        path: "app.js",
        content_type: "text/javascript; charset=utf-8",
        bytes: b"1",
    },
];

fn config() -> ServerConfig {
    ServerConfig {
        devtools: Some(DevtoolsConfig::new(TOKEN, ASSETS)),
        ..quick()
    }
}

fn start_devtools() -> Fixture {
    start_with(config(), "dev")
}

/// One HTTP request, answered with the status line and the body.
fn http_get(fx: &Fixture, target: &str) -> (String, String) {
    let addr = fx.server.addr();
    let mut tcp = TcpStream::connect(addr).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        tcp,
        "GET {target} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut text = String::new();
    tcp.read_to_string(&mut text).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    (head.to_owned(), body.to_owned())
}

/// A devtools page as a raw WebSocket client.
struct Page {
    ws: WebSocket<TcpStream>,
    seen: Vec<ServerMsg>,
    /// What has arrived and not been asked for yet.
    pending: Vec<ServerMsg>,
    /// The highest step `step()` has returned: a step may be sent twice (a page that attaches is
    /// told the steps it may already have), and a page applies them idempotently.
    last_step: u32,
}

impl Page {
    #[allow(clippy::result_large_err)]
    fn try_connect(
        fx: &Fixture,
        query: &str,
        origin: Option<&str>,
    ) -> Result<Page, tungstenite::Error> {
        use tungstenite::client::IntoClientRequest;
        let addr = fx.server.addr();
        let tcp = TcpStream::connect(addr).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut request = format!("ws://{addr}/devtools/ws{query}")
            .into_client_request()
            .unwrap();
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert("Origin", origin.parse().unwrap());
        }
        let (ws, _) = tungstenite::client(request, tcp).map_err(|e| match e {
            tungstenite::HandshakeError::Failure(e) => e,
            tungstenite::HandshakeError::Interrupted(_) => unreachable!("blocking socket"),
        })?;
        Ok(Page {
            ws,
            seen: Vec::new(),
            pending: Vec::new(),
            last_step: 0,
        })
    }

    fn connect(fx: &Fixture) -> Page {
        Page::try_connect(fx, &format!("?token={TOKEN}"), None)
            .expect("the devtools socket upgrades")
    }

    fn send(&mut self, msg: ClientMsg) {
        let mut w = undra::wire::Writer::new();
        msg.encode(&mut w);
        self.ws.send(Message::Binary(w.into_vec())).unwrap();
    }

    /// The next message, or `None` after `timeout`.
    fn next_within(&mut self, timeout: Duration) -> Option<ServerMsg> {
        let _ = self.ws.get_ref().set_read_timeout(Some(timeout));
        loop {
            match self.ws.read() {
                Ok(Message::Binary(bytes)) => {
                    let msg = ServerMsg::decode(&bytes).expect("the server speaks the protocol");
                    self.seen.push(msg.clone());
                    return Some(msg);
                }
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(Message::Text(t)) => panic!("text from the server: {t}"),
            }
        }
    }

    /// The first message, anywhere in what has arrived and what arrives within 5 seconds, that
    /// `pick` takes. The messages it skips stay for the next call: a page receives a change-set
    /// before the answer to the request that caused it, and a test asks for them in any order.
    fn until<T>(&mut self, what: &str, mut pick: impl FnMut(&ServerMsg) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            for i in 0..self.pending.len() {
                if let Some(found) = pick(&self.pending[i]) {
                    self.pending.remove(i);
                    return found;
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.next_within(left.max(Duration::from_millis(1))) {
                Some(msg) => self.pending.push(msg),
                None => panic!("no {what} within 5 s; saw {} messages", self.seen.len()),
            }
        }
    }

    /// The next commit change-set, decoded, with its cause.
    fn commit(&mut self) -> (ChangeSet, Cause) {
        self.until("a commit change-set", |m| match m {
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
        })
    }

    /// The next step, as `(step, restored_from)`.
    fn step(&mut self) -> (u32, u32) {
        let after = self.last_step;
        let found = self.until("a step", |m| match m {
            ServerMsg::Step(s) if s.step > after => Some((s.step, s.restored_from)),
            _ => None,
        });
        self.last_step = found.0;
        found
    }
}

fn i32_of(entry_value: &[u8]) -> i32 {
    i32::decode_exact(entry_value).unwrap()
}

#[test]
fn the_page_and_its_socket_need_the_token_and_everything_else_is_a_404() {
    let fx = start_devtools();
    // The page, with the token put into it.
    let (head, body) = http_get(&fx, &format!("/devtools?token={TOKEN}"));
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.contains("Cache-Control: no-store"), "{head}");
    assert!(head.contains("Content-Security-Policy:"), "{head}");
    assert_eq!(body, format!("<html>{TOKEN}</html>"));
    let (head, body) = http_get(&fx, &format!("/devtools/app.js?token={TOKEN}"));
    assert!(
        head.starts_with("HTTP/1.1 200") && head.contains("text/javascript"),
        "{head}"
    );
    assert_eq!(body, "1");
    // Wrong, missing or unknown: the same 404.
    let mut answers = Vec::new();
    for target in [
        "/devtools".to_owned(),
        "/devtools?token=wrong".to_owned(),
        format!("/devtools/missing.js?token={TOKEN}"),
        format!("/devtools/../x?token={TOKEN}"),
    ] {
        let (head, body) = http_get(&fx, &target);
        assert!(head.starts_with("HTTP/1.1 404"), "{target}: {head}");
        answers.push((head, body));
    }
    assert!(
        answers.windows(2).all(|w| w[0] == w[1]),
        "404s differ: {answers:?}"
    );
    // The socket: no token, a wrong one.
    for query in ["", "?token=wrong", "?token="] {
        match Page::try_connect(&fx, query, None) {
            Err(tungstenite::Error::Http(response)) => {
                assert_eq!(response.status(), 404, "{query}")
            }
            other => panic!(
                "{query}: expected a 404, got {:?}",
                other.map(|_| "a socket")
            ),
        }
    }
    // Nothing of this woke the hub.
    assert!(!fx.bridge.is_connected());
}

#[test]
fn without_devtools_configured_every_devtools_path_is_a_404() {
    let fx = start_with(quick(), "dev");
    let (head, _) = http_get(&fx, &format!("/devtools?token={TOKEN}"));
    assert!(head.starts_with("HTTP/1.1 404"), "{head}");
    assert!(Page::try_connect(&fx, &format!("?token={TOKEN}"), None).is_err());
    // An app client is untouched.
    let mut app = fx.client();
    assert!(app.new_counter(1) > 0);
}

#[test]
fn a_token_that_is_too_short_turns_devtools_off_and_says_so() {
    let cfg = ServerConfig {
        devtools: Some(DevtoolsConfig::new("short", ASSETS)),
        ..quick()
    };
    let fx = start_with(cfg, "dev");
    let (head, _) = http_get(&fx, "/devtools?token=short");
    assert!(head.starts_with("HTTP/1.1 404"), "{head}");
}

#[test]
fn a_page_from_an_origin_that_is_not_allowed_is_refused() {
    let fx = start_devtools();
    let refused = Page::try_connect(
        &fx,
        &format!("?token={TOKEN}"),
        Some("https://evil.example"),
    );
    assert!(matches!(refused, Err(tungstenite::Error::Http(r)) if r.status() == 404));
    assert!(
        Page::try_connect(
            &fx,
            &format!("?token={TOKEN}"),
            Some("http://localhost:5173")
        )
        .is_ok()
    );
}

#[test]
fn an_attached_page_gets_the_schema_the_stores_and_the_first_step() {
    let fx = start_devtools();
    let mut app = fx.client();
    let counter = app.new_counter(5);
    let mut page = Page::connect(&fx);
    let ServerMsg::Welcome(w) = page.next_within(Duration::from_secs(5)).unwrap() else {
        panic!("the first message is the welcome");
    };
    assert_eq!(w.schema_hash, fx.schema());
    assert_eq!(w.mode, "dev");
    assert!(
        w.schema_json.contains("\"Counter\""),
        "the schema is in the welcome"
    );
    assert!(w.ring_steps > 0);
    // The store exists, the app observed nothing of it, and the page has its values anyway.
    let stores = page.until("the stores", |m| match m {
        ServerMsg::Stores(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    });
    assert_eq!(stores.len(), 1);
    assert_eq!((stores[0].handle, stores[0].type_id), (counter, COUNTER));
    let (initial, _) = page.until("the initial values", |m| match m {
        ServerMsg::ChangeSet {
            delivery: Delivery::Initial,
            payload,
            ..
        } => Some((ChangeSet::decode(&mut Reader::new(payload)).unwrap(), ())),
        _ => None,
    });
    let count = initial
        .entries
        .iter()
        .find(|e| e.signal_id == COUNT_SIGNAL)
        .expect("count is there");
    assert_eq!((count.op, i32_of(&count.value)), (ChangeOp::Full, 5));
    assert_eq!(page.step().0, 1, "the state it attached to is step 1");
    page.until("the app", |m| {
        matches!(
            m,
            ServerMsg::App {
                connected: true,
                ..
            }
        )
        .then_some(())
    });
    page.until("the counters", |m| {
        matches!(m, ServerMsg::Stats(j) if j.contains("\"core\":{")).then_some(())
    });
}

#[test]
fn a_page_sees_every_signal_and_the_app_only_what_it_observed() {
    let fx = start_devtools();
    let mut app = fx.client();
    let counter = app.new_counter(0);
    app.observe(counter, COUNT_SIGNAL, true);
    let first = app.recv_kind(Kind::ChangeSet);
    assert_eq!(change_set(&first).entries.len(), 1);

    let mut page = Page::connect(&fx);
    page.step();
    // One transaction writes `count` and `label`; the app watches `count` only.
    let before = change_sets(&app).len();
    let (status, _) = app.method(counter, ADD_AND_LABEL, &enc(&7_i32));
    assert_eq!(status, undra::wire::payload::ReplyStatus::Ok);
    let (set, cause) = page.commit();
    let mut signals: Vec<u32> = set.entries.iter().map(|e| e.signal_id).collect();
    signals.sort_unstable();
    assert_eq!(
        signals,
        [0, 1],
        "the page sees both signals of the transaction"
    );
    assert_eq!(
        cause,
        Cause::Call(ADD_AND_LABEL),
        "and which call caused them"
    );

    let sets = change_sets(&app);
    assert_eq!(sets.len(), before + 1);
    let mine = sets.last().unwrap();
    assert_eq!(mine.txn_id, set.txn_id);
    assert_eq!(
        mine.entries.iter().map(|e| e.signal_id).collect::<Vec<_>>(),
        [COUNT_SIGNAL],
        "the app client gets the entry it observed and not the other"
    );
    // The page's attach did not send the app anything it had not asked for.
    assert!(app.silent_for(Duration::from_millis(150)));
}

#[test]
fn the_app_unobserving_does_not_blind_the_page_and_the_page_leaving_restores_the_app_set() {
    let fx = start_devtools();
    let mut app = fx.client();
    let counter = app.new_counter(0);
    app.observe(counter, COUNT_SIGNAL, true);
    app.recv_kind(Kind::ChangeSet);
    let mut page = Page::connect(&fx);
    page.step();

    app.observe(counter, COUNT_SIGNAL, false);
    let before = change_sets(&app).len();
    app.method(counter, ADD, &enc(&1_i32));
    let (set, _) = page.commit();
    assert_eq!(
        i32_of(&set.entries[0].value),
        1,
        "the page still sees what the app stopped watching"
    );
    assert!(
        app.silent_for(Duration::from_millis(150)),
        "and the app does not"
    );
    assert_eq!(change_sets(&app).len(), before);

    // The app watches `count` again; the page leaves; the runtime observes what the app asked for.
    app.observe(counter, COUNT_SIGNAL, true);
    app.recv_kind(Kind::ChangeSet);
    drop(page);
    fx.eventually("the hub to let go", |fx| !fx.bridge.devtools_attached());
    // The runtime's observed set is the app's own again, and the client is sent the current value
    // of what it observes, like any `Observe`.
    let restated = change_set(&app.recv_kind(Kind::ChangeSet));
    assert_eq!(
        restated
            .entries
            .iter()
            .map(|e| e.signal_id)
            .collect::<Vec<_>>(),
        [COUNT_SIGNAL]
    );
    let before = change_sets(&app).len();
    app.method(counter, ADD_AND_LABEL, &enc(&1_i32));
    let sets = change_sets(&app);
    assert_eq!(sets.len(), before + 1);
    assert_eq!(
        sets.last()
            .unwrap()
            .entries
            .iter()
            .map(|e| e.signal_id)
            .collect::<Vec<_>>(),
        [COUNT_SIGNAL]
    );
    // The label write was not observed: nothing else follows.
    assert!(app.silent_for(Duration::from_millis(150)));
}

#[test]
fn time_travel_restores_the_core_and_the_app_converges() {
    let fx = start_devtools();
    let mut app = fx.client();
    let counter = app.new_counter(5);
    app.observe(counter, COUNT_SIGNAL, true);
    app.recv_kind(Kind::ChangeSet);
    let mut page = Page::connect(&fx);
    let (first, _) = page.step();
    assert_eq!(first, 1);

    app.method(counter, ADD, &enc(&3_i32));
    let (second, _) = page.step();
    assert_eq!(second, 2);
    assert_eq!(i32_of(&app.method(counter, GET, &[]).1), 8);
    assert_eq!(
        i32_of(&change_sets(&app).last().unwrap().entries[0].value),
        8
    );

    page.send(ClientMsg::Restore {
        request_id: 41,
        step: 1,
    });
    let traveled = page.until("the answer", |m| match m {
        ServerMsg::Traveled(t) => Some(t.clone()),
        _ => None,
    });
    assert!(
        (
            traveled.request_id,
            traveled.ok,
            traveled.step,
            traveled.dropped
        ) == (41, true, 1, 0),
        "{traveled:?}"
    );

    // The app's mirror converges through the change-set the restore emits, and the dev bar is told.
    let restored = change_set(&app.recv_kind(Kind::ChangeSet));
    assert_eq!(i32_of(&restored.entries[0].value), 5);
    let notice = loop {
        let log = app.recv_kind(Kind::Log);
        let log = Log::decode(&mut Reader::new(&log.payload)).unwrap();
        if log.target == "undra::dev" {
            break log.message.to_owned();
        }
    };
    assert_eq!(notice, "time travel: step 1");
    assert_eq!(
        i32_of(&app.method(counter, GET, &[]).1),
        5,
        "the core really is at step 1"
    );

    // The page: the restore is a labelled commit and a new step that says where it came from.
    let set = page.until("the commit of the restore", |m| match m {
        ServerMsg::ChangeSet {
            delivery: Delivery::Commit,
            cause: Cause::Restore(1),
            payload,
            ..
        } => Some(ChangeSet::decode(&mut Reader::new(payload)).unwrap()),
        _ => None,
    });
    assert_eq!(
        i32_of(&set.entries.iter().find(|e| e.signal_id == 0).unwrap().value),
        5
    );
    let (third, restored_from) = page.until("the step of the restore", |m| match m {
        ServerMsg::Step(s) if s.restored_from != 0 => Some((s.step, s.restored_from)),
        _ => None,
    });
    assert_eq!((third, restored_from), (3, 1), "history is append-only");
}

#[test]
fn travelling_to_a_step_that_is_gone_is_refused_with_a_reason() {
    let fx = start_devtools();
    let mut page = Page::connect(&fx);
    page.step();
    page.send(ClientMsg::Restore {
        request_id: 1,
        step: 999,
    });
    let t = page.until("the answer", |m| match m {
        ServerMsg::Traveled(t) => Some(t.clone()),
        _ => None,
    });
    assert!(!t.ok && t.message.contains("999"), "{t:?}");
}

#[test]
fn restoring_a_step_from_before_a_store_existed_says_the_store_is_dropped() {
    let fx = start_devtools();
    let mut page = Page::connect(&fx);
    page.step(); // step 1: no store yet
    let mut app = fx.client();
    let counter = app.new_counter(1);
    page.until("the store", |m| {
        matches!(m, ServerMsg::Stores(s) if !s.is_empty()).then_some(())
    });
    page.step();
    page.send(ClientMsg::Restore {
        request_id: 2,
        step: 1,
    });
    let t = page.until("the answer", |m| match m {
        ServerMsg::Traveled(t) => Some(t.clone()),
        _ => None,
    });
    assert!(t.ok && t.dropped == 1, "{t:?}");
    assert!(t.message.contains("stale"), "{t:?}");
    let (status, _) = app.method(counter, GET, &[]);
    assert_eq!(
        status,
        undra::wire::payload::ReplyStatus::BadRequest,
        "the app's handle is stale now"
    );
    // The app is told how many, through the dev notice its status bar shows.
    drain(&mut app);
    let notices: Vec<String> = app
        .frames_of(Kind::Log)
        .iter()
        .filter_map(|f| Log::decode(&mut Reader::new(&f.payload)).ok())
        .filter(|log| log.target == "undra::dev")
        .map(|log| log.message.to_owned())
        .collect();
    assert_eq!(notices, ["time travel: step 1 (1 store(s) built since are gone)"]);
}

#[test]
fn port_calls_are_recorded_with_arguments_reply_and_latency() {
    let fx = start_devtools();
    let mut app = fx.client();
    let counter = app.new_counter(0);
    let mut page = Page::connect(&fx);
    page.step();

    let id = app.next_call_id();
    app.send_call(
        undra::wire::payload::CallTarget::Method {
            handle: undra::wire::Handle(counter),
            method_id: ASK,
        },
        id,
        &enc(&21_i32),
    );
    let frame = app.recv_port_call(ECHO_PORT);
    let call = PortCall::decode(&mut Reader::new(&frame.payload)).unwrap();
    let (port_call_id, args) = (call.port_call_id, call.args.to_vec());
    let start = page.until("the start of the port call", |m| match m {
        ServerMsg::Port(PortRecord::Start {
            id, port_id, args, ..
        }) if *port_id == ECHO_PORT => Some((*id, args.clone())),
        _ => None,
    });
    assert_eq!(start, (port_call_id, args));
    assert_eq!(i32_of(&start.1), 21);

    std::thread::sleep(Duration::from_millis(30));
    app.port_reply(port_call_id, PortStatus::Ok, &enc(&42_i32));
    let end = page.until("the end of the port call", |m| match m {
        ServerMsg::Port(PortRecord::End {
            id,
            status,
            latency_us,
            reply,
            method_id,
            ..
        }) if *id == port_call_id => Some((*status, *latency_us, reply.clone(), *method_id)),
        _ => None,
    });
    assert_eq!(end.0, 0);
    assert!(end.1 >= 30_000, "the latency covers the wait: {} us", end.1);
    assert_eq!(i32_of(&end.2), 42);
    assert_eq!(end.3, ECHO_METHOD);
    assert_eq!(i32_of(&app.await_reply(id).1), 42);
}

#[test]
fn a_port_call_with_no_app_client_is_counted_and_not_listed() {
    let fx = start_devtools();
    let mut page = Page::connect(&fx);
    page.step();
    // No app client: the core's own call to a platform port ends at once. Such calls repeat (a
    // query's hydration retries), so the page is told how many, not each one.
    on_core(&fx.rt, {
        let rt = fx.rt.clone();
        move || {
            for _ in 0..3 {
                drop(rt.ctx().port_call(ECHO_PORT, ECHO_METHOD, enc(&1_i32)));
            }
        }
    });
    let unattended = page.until("the counters to say so", |m| match m {
        ServerMsg::Stats(json) => {
            let stats: serde_json::Value = serde_json::from_str(json).unwrap();
            let n = stats["server"]["unattended_port_calls"].as_u64().unwrap();
            (n >= 3).then_some(n)
        }
        _ => None,
    });
    assert!(unattended >= 3);
    assert!(
        !page.seen.iter().any(|m| matches!(m, ServerMsg::Port(PortRecord::End { port_id, .. }) if *port_id == ECHO_PORT)),
        "nobody was asked, so there is nothing to list"
    );
}

#[test]
fn a_second_page_gets_the_current_state_and_the_history() {
    let fx = start_devtools();
    let mut app = fx.client();
    let counter = app.new_counter(1);
    let mut first = Page::connect(&fx);
    first.step();
    app.method(counter, ADD, &enc(&1_i32));
    assert_eq!(first.step().0, 2);

    let mut second = Page::connect(&fx);
    let one = second.step().0;
    let two = second.step().0;
    let steps = [one, two];
    assert_eq!(steps, [1, 2]);
    let value = second.until("the value", |m| match m {
        ServerMsg::ChangeSet {
            delivery: Delivery::Initial,
            payload,
            ..
        } => {
            let set = ChangeSet::decode(&mut Reader::new(payload)).unwrap();
            set.entries
                .iter()
                .find(|e| e.signal_id == 0)
                .map(|e| i32_of(&e.value))
        }
        _ => None,
    });
    assert_eq!(value, 2);
}

#[test]
fn the_ring_is_bounded_and_the_page_is_told_what_was_dropped() {
    let mut devtools = DevtoolsConfig::new(TOKEN, ASSETS);
    devtools.max_steps = 3;
    let fx = start_with(
        ServerConfig {
            devtools: Some(devtools),
            ..quick()
        },
        "dev",
    );
    let mut app = fx.client();
    let counter = app.new_counter(0);
    let mut page = Page::connect(&fx);
    page.step();
    for _ in 0..5 {
        app.method(counter, ADD, &enc(&1_i32));
        // One step per call: wait for it so the commits are not coalesced.
        page.step();
    }
    let below = page.until("an eviction", |m| match m {
        ServerMsg::Evicted { below_step } => Some(*below_step),
        _ => None,
    });
    assert!(below >= 2, "{below}");
    page.send(ClientMsg::Restore {
        request_id: 1,
        step: 1,
    });
    let t = page.until("the answer", |m| match m {
        ServerMsg::Traveled(t) => Some(t.clone()),
        _ => None,
    });
    assert!(!t.ok, "an evicted step cannot be restored: {t:?}");
}

#[test]
fn a_state_over_the_per_step_limit_is_listed_but_not_restorable() {
    let mut devtools = DevtoolsConfig::new(TOKEN, ASSETS);
    devtools.max_step_bytes = 8;
    let fx = start_with(
        ServerConfig {
            devtools: Some(devtools),
            ..quick()
        },
        "dev",
    );
    let mut app = fx.client();
    app.new_counter(0);
    let mut page = Page::connect(&fx);
    let info = page.until("a step", |m| match m {
        ServerMsg::Step(s) => Some(s.clone()),
        _ => None,
    });
    assert!(!info.restorable && info.bytes > 8, "{info:?}");
    page.send(ClientMsg::Restore {
        request_id: 1,
        step: info.step,
    });
    let t = page.until("the answer", |m| match m {
        ServerMsg::Traveled(t) => Some(t.clone()),
        _ => None,
    });
    assert!(!t.ok && t.message.contains("limit"), "{t:?}");
}

#[test]
fn a_malformed_message_closes_that_page_and_nothing_else() {
    let fx = start_devtools();
    let mut app = fx.client();
    app.new_counter(0);
    let mut page = Page::connect(&fx);
    page.step();
    page.ws.send(Message::Binary(vec![0xee, 1, 2])).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while page.next_within(Duration::from_millis(200)).is_some() {
        assert!(Instant::now() < deadline, "the page was never closed");
    }
    // The core and the app client are fine.
    let counter = app.new_counter(2);
    assert_eq!(i32_of(&app.method(counter, GET, &[]).1), 2);
}

#[test]
fn suspending_the_server_closes_the_pages_and_leaves_the_core_unobserved() {
    let fx = start_devtools();
    let mut app = fx.client();
    let counter = app.new_counter(3);
    let mut page = Page::connect(&fx);
    page.step();
    let suspended = fx.server.suspend(Duration::from_millis(200));
    assert!(suspended.settled);
    // The page was closed (1001), not left to time out.
    let deadline = Instant::now() + Duration::from_secs(5);
    while page.next_within(Duration::from_millis(200)).is_some() {
        assert!(Instant::now() < deadline, "the page was never closed");
    }
    // The core is still there, with its state, for the snapshot a reload takes.
    let snapshot = fx.rt.snapshot();
    assert!(!snapshot.is_empty());
    let _ = counter;
}

#[test]
fn the_page_leaving_clears_the_history_and_a_new_page_starts_again_at_step_one() {
    let fx = start_devtools();
    let mut app = fx.client();
    let counter = app.new_counter(0);
    let mut page = Page::connect(&fx);
    page.step();
    app.method(counter, ADD, &enc(&1_i32));
    page.step();
    drop(page);
    fx.eventually("the hub to let go", |fx| !fx.bridge.devtools_attached());
    let mut again = Page::connect(&fx);
    // The ring was cleared; step numbers keep counting within the process.
    let (n, _) = again.step();
    assert!(n >= 3, "a number is never reused: {n}");
}

// ----- adversarial review (2026-10-02) -------------------------------------------------------

/// What a raw request is answered with, byte for byte (until the server closes).
fn raw(fx: &Fixture, request: &str) -> Vec<u8> {
    let mut tcp = TcpStream::connect(fx.server.addr()).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    tcp.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = tcp.read_to_end(&mut out);
    out
}

const UPGRADE: &str = "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n";

#[test]
fn every_refusal_is_the_same_bytes_and_a_server_with_devtools_off_answers_the_same() {
    let on = start_devtools();
    let off = start_with(quick(), "dev");
    let wrong = "t0k3nt0k3nt0k3nZY";
    let requests = [
        "GET /devtools HTTP/1.1\r\nHost: x\r\n\r\n".to_owned(),
        format!("GET /devtools?token={wrong} HTTP/1.1\r\nHost: x\r\n\r\n"),
        format!("GET /devtools?token={TOKEN}x HTTP/1.1\r\nHost: x\r\n\r\n"),
        format!("GET /devtools?token={} HTTP/1.1\r\nHost: x\r\n\r\n", &TOKEN[..8]),
        format!("GET /devtools/missing.js?token={TOKEN} HTTP/1.1\r\nHost: x\r\n\r\n"),
        format!("GET /devtools/%2e%2e/x?token={TOKEN} HTTP/1.1\r\nHost: x\r\n\r\n"),
        format!("POST /devtools?token={TOKEN} HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n"),
        format!("HEAD /devtools?token={TOKEN} HTTP/1.1\r\nHost: x\r\n\r\n"),
        "GET /devtools/ws HTTP/1.1\r\nHost: x\r\n\r\n".to_owned(),
        format!("GET /devtools/ws HTTP/1.1\r\nHost: x\r\n{UPGRADE}\r\n"),
        format!("GET /devtools/ws?token={wrong} HTTP/1.1\r\nHost: x\r\n{UPGRADE}\r\n"),
        format!("GET /devtools/ws?token= HTTP/1.1\r\nHost: x\r\n{UPGRADE}\r\n"),
        format!("GET /devtools/ws?token={wrong} HTTP/1.1\r\nHost: x\r\nOrigin: https://evil.example\r\n{UPGRADE}\r\n"),
        format!("POST /devtools/ws?token={wrong} HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n"),
        "GET /devtools/ws/ HTTP/1.1\r\nHost: x\r\n\r\n".to_owned(),
    ];
    let reference = raw(&off, &requests[0]);
    assert!(reference.starts_with(b"HTTP/1.1 404"), "{}", String::from_utf8_lossy(&reference));
    for request in &requests {
        for (name, fx) in [("devtools on", &on), ("devtools off", &off)] {
            assert_eq!(
                raw(fx, request),
                reference,
                "{name}: {request:?} is not answered with the one 404"
            );
        }
    }
    // Nothing of this woke the hub, and no connection is left behind.
    assert!(!on.bridge.devtools_attached() && !on.bridge.is_connected());
}

/// The first message of a page connection, or whether it was closed.
fn closed_within(page: &mut Page, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        if page.next_within(Duration::from_millis(100)).is_none() {
            // A read timeout is not a close: ask again with a write.
            if page.ws.send(Message::Ping(vec![1].into())).is_err() {
                return true;
            }
        }
    }
    false
}

#[test]
fn a_page_cannot_reach_the_core_through_its_socket_whatever_it_sends() {
    let fx = start_devtools();
    let mut app = fx.client();
    let counter = app.new_counter(10);
    app.observe(counter, COUNT_SIGNAL, true);
    app.recv_kind(Kind::ChangeSet);
    let value = |app: &mut TestClient| i32_of(&app.method(counter, GET, &[]).1);
    let before = fx.rt.stats_json();
    let calls_before = stat(&fx.rt, "calls");

    // An app envelope (a Call that would add 100): not a message of this protocol.
    let mut w = undra::wire::Writer::new();
    undra::wire::Envelope::write(
        &mut w,
        Kind::Call,
        1,
        fx.schema(),
        &undra::runtime::testing::call_payload(
            undra::wire::payload::CallTarget::Method {
                handle: undra::wire::Handle(counter),
                method_id: ADD,
            },
            77,
            &enc(&100_i32),
        ),
    );
    let envelope = w.into_vec();
    let mut shapes: Vec<Vec<u8>> = vec![
        envelope,
        vec![],
        vec![0],
        vec![1],
        vec![1, 0, 0, 0],
        vec![1, 1, 0, 0, 0, 1, 0, 0],
        vec![1, 1, 0, 0, 0, 1, 0, 0, 0, 9],
        vec![2, 0],
        vec![2, 2],
        vec![0xff; 9],
        vec![0xff; 1000],
    ];
    // Every tag byte, with no body, a short body, and a body of the right size for a restore.
    for tag in 0..=255_u8 {
        shapes.push(vec![tag]);
        shapes.push(vec![tag, 0xff, 0xff, 0xff]);
        shapes.push(vec![tag, 5, 0, 0, 0, 1, 0, 0, 0]);
    }
    let mut closed = 0;
    for shape in &shapes {
        let mut page = Page::connect(&fx);
        page.ws.send(Message::Binary(shape.clone().into())).unwrap();
        // A message that is a valid `Resync` (tag 2, no body) or a `Restore` of a step that is
        // not there keeps the page; every other one closes it. Neither may touch the core.
        let valid = matches!(shape.as_slice(), [2] | [1, 5, 0, 0, 0, 1, 0, 0, 0]);
        if valid {
            continue;
        }
        assert!(
            closed_within(&mut page, Duration::from_secs(5)),
            "{shape:02x?} did not close the page"
        );
        closed += 1;
    }
    assert!(closed > 700, "{closed}");
    // A text frame and an oversize message are refused too.
    let mut page = Page::connect(&fx);
    page.ws.send(Message::Text("restore 1".into())).unwrap();
    assert!(closed_within(&mut page, Duration::from_secs(5)));
    let mut page = Page::connect(&fx);
    let _ = page.ws.send(Message::Binary(vec![1; 200 * 1024].into()));
    assert!(closed_within(&mut page, Duration::from_secs(5)));

    // The core and the app client did not notice: nothing was called, nothing was restored.
    assert_eq!(value(&mut app), 10, "the Call envelope was not executed");
    assert_eq!(
        stat(&fx.rt, "calls") - calls_before,
        1,
        "only the app's own `get` was a call ({before})"
    );
    assert!(app.silent_for(Duration::from_millis(100)));
    // The hub is intact: a page still attaches and sees the state.
    let mut page = Page::connect(&fx);
    assert_eq!(page.step().0 >= 1, true);
    assert!(
        fx.log_lines().iter().all(|l| !l.contains("panicked")),
        "{:?}",
        fx.log_lines()
    );
}

// ----- observe-all routing, attacked --------------------------------------------------------

/// Every `(handle, signal)` the app client was ever sent a value for.
fn seen_by(app: &TestClient) -> std::collections::BTreeSet<(u64, u32)> {
    change_sets(app)
        .iter()
        .flat_map(|set| set.entries.iter().map(|e| (e.handle.0, e.signal_id)))
        .collect()
}

/// Reads everything the app client is sent until it has been quiet for a moment.
fn drain(app: &mut TestClient) {
    while !app.silent_for(Duration::from_millis(120)) {}
}

/// The next commit of the page that writes store `handle`.
fn commit_of(page: &mut Page, handle: u64) -> ChangeSet {
    page.until("a commit of the store", |m| match m {
        ServerMsg::ChangeSet {
            delivery: Delivery::Commit,
            payload,
            ..
        } => {
            let set = ChangeSet::decode(&mut Reader::new(payload)).unwrap();
            set.entries
                .iter()
                .any(|e| e.handle.0 == handle)
                .then_some(set)
        }
        _ => None,
    })
}

#[test]
fn the_app_gets_exactly_what_it_observed_before_during_and_after_a_page() {
    let fx = start_devtools();
    let mut app = fx.client();
    let a = app.new_counter(0);
    let b = app.new_counter(100);
    app.observe(a, COUNT_SIGNAL, true);
    app.recv_kind(Kind::ChangeSet);
    app.method(b, ADD, &enc(&1_i32)); // b = 101, nobody watches
    assert!(app.silent_for(Duration::from_millis(100)));

    let mut page = Page::connect(&fx);
    page.step();
    // B changes: the page sees it, the app does not. A changes: the app sees only A's `count`.
    app.method(b, ADD, &enc(&1_i32)); // 102
    assert_eq!(i32_of(&commit_of(&mut page, b).entries[0].value), 102);
    app.method(a, ADD_AND_LABEL, &enc(&5_i32));
    commit_of(&mut page, a);
    drain(&mut app);
    assert_eq!(
        seen_by(&app).into_iter().collect::<Vec<_>>(),
        [(a, COUNT_SIGNAL)],
        "the app was sent A's observed signal and nothing of B or of A's label"
    );

    // The page leaves: the app's set is its own again, and B stays unseen by it.
    drop(page);
    fx.eventually("the hub to let go", |fx| !fx.bridge.devtools_attached());
    app.method(b, ADD, &enc(&1_i32)); // 103
    app.method(a, ADD_AND_LABEL, &enc(&1_i32));
    drain(&mut app);
    assert_eq!(
        seen_by(&app).into_iter().collect::<Vec<_>>(),
        [(a, COUNT_SIGNAL)],
        "nothing of B reached the app after the page left either"
    );
    // The app observes B now and is told its current value, and then its changes.
    app.observe(b, COUNT_SIGNAL, true);
    let mut seen_b = None;
    while seen_b.is_none() {
        let set = change_set(&app.recv_kind(Kind::ChangeSet));
        seen_b = set
            .entries
            .iter()
            .find(|e| e.handle.0 == b)
            .map(|e| i32_of(&e.value));
    }
    assert_eq!(seen_b, Some(103));
}

#[test]
fn two_pages_share_the_observation_and_the_last_one_out_restores_the_app() {
    let fx = start_devtools();
    let mut app = fx.client();
    let a = app.new_counter(0);
    let b = app.new_counter(0);
    app.observe(a, COUNT_SIGNAL, true);
    app.recv_kind(Kind::ChangeSet);
    let mut one = Page::connect(&fx);
    one.step();
    let mut two = Page::connect(&fx);
    two.step();

    drop(one);
    // One page is still attached: the hub stays active, B is still observed for it, and the app
    // still gets only its own.
    std::thread::sleep(Duration::from_millis(100));
    assert!(fx.bridge.devtools_attached());
    app.method(b, ADD, &enc(&7_i32));
    assert_eq!(i32_of(&commit_of(&mut two, b).entries[0].value), 7);
    assert_eq!(
        seen_by(&app).into_iter().collect::<Vec<_>>(),
        [(a, COUNT_SIGNAL)]
    );

    drop(two);
    fx.eventually("the hub to let go", |fx| !fx.bridge.devtools_attached());
    app.method(b, ADD, &enc(&1_i32));
    app.method(a, ADD, &enc(&1_i32));
    // Whatever order the restatement and the write arrive in, B is never among it.
    drain(&mut app);
    assert_eq!(
        seen_by(&app).into_iter().collect::<Vec<_>>(),
        [(a, COUNT_SIGNAL)]
    );
}

fn resuming_config() -> ServerConfig {
    ServerConfig {
        resume_grace: Duration::from_secs(600),
        ..config()
    }
}

#[test]
fn an_app_that_reconnects_with_a_page_attached_is_sent_only_what_it_asks_for() {
    let fx = start_with(resuming_config(), "dev");
    let mut app = fx.session_client("tok-dt", false);
    let a = app.new_counter(1);
    let b = app.new_counter(50);
    app.observe(a, COUNT_SIGNAL, true);
    app.recv_kind(Kind::ChangeSet);
    let mut page = Page::connect(&fx);
    page.step();

    // The app drops and comes back with its session while the page is attached.
    drop(app);
    fx.eventually("the slot is free", |fx| !fx.bridge.is_connected());
    page.until("the app to be gone", |m| {
        matches!(m, ServerMsg::App { connected: false, .. }).then_some(())
    });
    let mut app = fx.session_client("tok-dt", true);
    page.until("the app to be back", |m| {
        matches!(m, ServerMsg::App { connected: true, .. }).then_some(())
    });
    // Before it observes anything the app is sent nothing, though the hub observes every store.
    app.method(b, ADD, &enc(&1_i32));
    app.method(a, ADD, &enc(&1_i32));
    assert!(app.silent_for(Duration::from_millis(150)));
    assert!(seen_by(&app).is_empty(), "{:?}", seen_by(&app));
    // The page saw both writes.
    commit_of(&mut page, b);
    commit_of(&mut page, a);
    // The app observes A: it is told A's value and not B's.
    app.observe(a, COUNT_SIGNAL, true);
    app.recv_kind(Kind::ChangeSet);
    app.method(b, ADD, &enc(&1_i32));
    commit_of(&mut page, b);
    assert_eq!(
        seen_by(&app).into_iter().collect::<Vec<_>>(),
        [(a, COUNT_SIGNAL)]
    );
    // The page leaves: what the app asked for stays observed, and only that.
    drop(page);
    fx.eventually("the hub to let go", |fx| !fx.bridge.devtools_attached());
    app.method(b, ADD, &enc(&1_i32));
    app.method(a, ADD, &enc(&1_i32));
    drain(&mut app);
    assert_eq!(
        seen_by(&app).into_iter().collect::<Vec<_>>(),
        [(a, COUNT_SIGNAL)]
    );
}

#[test]
fn a_reload_with_a_page_attached_hands_the_app_session_over_without_the_hubs_observations() {
    let fx = start_with(resuming_config(), "dev");
    let addr = fx.server.addr();
    let mut app = fx.session_client("tok-reload-dt", false);
    let a = app.new_counter(1);
    let b = app.new_counter(50);
    app.observe(a, COUNT_SIGNAL, true);
    app.recv_kind(Kind::ChangeSet);
    let mut page = Page::connect(&fx);
    let ServerMsg::Welcome(first_epoch) = page.next_within(Duration::from_secs(5)).unwrap() else {
        panic!("the welcome comes first");
    };
    page.step();

    let suspended = fx.server.suspend(Duration::from_millis(500));
    assert!(suspended.settled);
    let session = suspended.session.expect("the app's session is handed over");
    assert_eq!(session.token, "tok-reload-dt");
    let mut handles = session.handles.clone();
    handles.sort_unstable();
    assert_eq!(handles, [a, b], "handles, and nothing the hub took");
    let snapshot = fx.rt.snapshot();
    drop(page);
    drop(app);
    drop(fx);

    // The new core: restored, with the session. It has no page yet, so nothing is observed that
    // the app has not asked for again.
    let second = restarted(addr, &snapshot, session);
    let mut back = second.session_client("tok-reload-dt", true);
    back.method(b, ADD, &enc(&1_i32));
    back.method(a, ADD, &enc(&1_i32));
    assert!(
        back.silent_for(Duration::from_millis(200)),
        "the app is sent nothing until it observes again"
    );
    assert!(seen_by(&back).is_empty(), "{:?}", seen_by(&back));
    back.observe(a, COUNT_SIGNAL, true);
    back.recv_kind(Kind::ChangeSet);
    assert_eq!(
        seen_by(&back).into_iter().collect::<Vec<_>>(),
        [(a, COUNT_SIGNAL)]
    );
    // A page of the new core is a new hub: another epoch, and the same rules.
    let mut page = Page::connect(&second);
    let ServerMsg::Welcome(second_epoch) = page.next_within(Duration::from_secs(5)).unwrap() else {
        panic!("the welcome comes first");
    };
    assert_ne!(first_epoch.core_epoch, second_epoch.core_epoch);
    page.step();
    back.method(b, ADD, &enc(&1_i32));
    commit_of(&mut page, b);
    assert_eq!(
        seen_by(&back).into_iter().collect::<Vec<_>>(),
        [(a, COUNT_SIGNAL)]
    );
}

/// A server on `addr` whose runtime was restored from `snapshot` before it listened, with the
/// session the old one handed over (what `undra dev` does at a reload).
fn restarted(
    addr: std::net::SocketAddr,
    snapshot: &[u8],
    session: undra_transport::KeptSession,
) -> Fixture {
    use undra::runtime::{Runtime, RuntimeConfig};
    let config = ServerConfig {
        inherited_session: Some(session),
        ..resuming_config()
    };
    let mut last = None;
    for _ in 0..50 {
        let (snapshot, config) = (snapshot.to_vec(), config.clone());
        match undra_transport::Server::start(addr, config, move |host| {
            let rt = Runtime::new(
                RuntimeConfig {
                    platform: "rust".into(),
                    mode: "dev".into(),
                    core_threads: 1,
                    blocking_threads: 1,
                    log_level: 0,
                },
                host,
            )?;
            rt.restore(&snapshot).expect("the snapshot restores");
            Ok(rt)
        }) {
            Ok(server) => {
                return Fixture {
                    rt: server.runtime().clone(),
                    bridge: server.bridge().clone(),
                    server,
                    logs: Default::default(),
                };
            }
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    panic!("the successor could not bind {addr}: {last:?}");
}

// ----- commit storms, bounds and cost, attacked -----------------------------------------------

/// Pipelines `n` `add(1)` calls on `counter` (a commit each) and waits for every reply.
fn storm(app: &mut TestClient, counter: u64, n: usize) {
    let mut sent = 0;
    while sent < n {
        let batch = 500.min(n - sent);
        let ids: Vec<u32> = (0..batch)
            .map(|_| {
                let id = app.next_call_id();
                app.send_call(
                    undra::wire::payload::CallTarget::Method {
                        handle: undra::wire::Handle(counter),
                        method_id: ADD,
                    },
                    id,
                    &enc(&1_i32),
                );
                id
            })
            .collect();
        for id in ids {
            assert_eq!(app.await_reply(id).0, undra::wire::payload::ReplyStatus::Ok);
        }
        sent += batch;
    }
}

/// Reads the page until it has been quiet for `quiet`.
fn drain_page(page: &mut Page, quiet: Duration) -> Vec<ServerMsg> {
    let mut got = Vec::new();
    while let Some(msg) = page.next_within(quiet) {
        got.push(msg);
    }
    got
}

#[test]
fn a_commit_storm_costs_steps_by_time_not_by_commit_and_the_ring_stays_bounded() {
    let fx = start_devtools();
    let mut app = fx.client();
    let counter = app.new_counter(0);
    let mut page = Page::connect(&fx);
    page.step();
    drain_page(&mut page, Duration::from_millis(200));

    const N: usize = 20_000;
    let started = Instant::now();
    storm(&mut app, counter, N);
    let storm_took = started.elapsed();
    let got = drain_page(&mut page, Duration::from_millis(700));
    let elapsed = started.elapsed();

    let commits = got
        .iter()
        .filter(|m| matches!(m, ServerMsg::ChangeSet { delivery: Delivery::Commit, .. }))
        .count();
    let steps: Vec<_> = got
        .iter()
        .filter_map(|m| match m {
            ServerMsg::Step(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    eprintln!(
        "storm: {N} commits in {storm_took:?} ({:.0}/s); the page saw {commits} change-sets and {} steps in {elapsed:?}",
        N as f64 / storm_took.as_secs_f64(),
        steps.len()
    );
    // Back-pressure is the page's own queue (below), so a page that keeps up misses nothing...
    assert_eq!(commits, N, "every commit reaches a page that reads");
    // ...while the snapshots are taken by time: a step is at most one per coalescing window, however
    // many commits there were. (The bound has slack for the first step and the final one.)
    let window_ms = u64::try_from(elapsed.as_millis()).unwrap();
    assert!(
        (steps.len() as u64) <= window_ms / 10 + 20,
        "{} steps in {window_ms} ms is more than one per 10 ms",
        steps.len()
    );
    assert!(steps.len() < N / 10, "{} steps for {N} commits", steps.len());
    // The page is not left behind: the last step covers the last commit.
    let last_seq = got
        .iter()
        .filter_map(|m| match m {
            ServerMsg::ChangeSet { seq, .. } => Some(*seq),
            _ => None,
        })
        .max()
        .unwrap();
    assert_eq!(steps.last().unwrap().through_seq, last_seq, "the newest step is the final state");
    // The ring stays within its bounds and says what it dropped.
    let stats = got.iter().rev().find_map(|m| match m {
        ServerMsg::Stats(j) => Some(serde_json::from_str::<serde_json::Value>(j).unwrap()),
        _ => None,
    });
    if let Some(stats) = stats {
        assert!(stats["server"]["ring_steps"].as_u64().unwrap() <= 200);
        assert!(stats["server"]["ring_bytes"].as_u64().unwrap() <= 32 << 20);
    }
    // The core is where the app left it, and a restore of the oldest kept step still works.
    assert_eq!(i32_of(&app.method(counter, GET, &[]).1), N as i32);
    let oldest = {
        let mut seen = steps.iter().map(|s| s.step).collect::<Vec<_>>();
        seen.sort_unstable();
        seen[seen.len().saturating_sub(150)]
    };
    page.send(ClientMsg::Restore { request_id: 9, step: oldest });
    let t = page.until("the answer", |m| match m {
        ServerMsg::Traveled(t) => Some(t.clone()),
        _ => None,
    });
    assert!(t.ok, "{t:?}");
}

#[test]
fn a_page_that_stops_reading_is_dropped_and_the_core_and_the_app_do_not_wait_for_it() {
    let fx = start_with(
        ServerConfig {
            max_queued_bytes: 128 * 1024,
            write_timeout: Duration::from_secs(30),
            ..config()
        },
        "dev",
    );
    let mut app = fx.client();
    let a = app.new_counter(0);
    let b = app.new_counter(0);
    app.observe(a, COUNT_SIGNAL, true);
    app.recv_kind(Kind::ChangeSet);
    let mut page = Page::connect(&fx);
    page.step();
    // The page does not read from here on.
    let started = Instant::now();
    storm(&mut app, b, 40_000);
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(60),
        "the app was held up by a page that does not read: {took:?}"
    );
    fx.eventually("the slow page to be dropped and the hub to let go", |fx| {
        !fx.bridge.devtools_attached()
    });
    // The app converged and is sent only its own signal; nothing of B reached it.
    assert_eq!(i32_of(&app.method(b, GET, &[]).1), 40_000);
    app.method(a, ADD, &enc(&1_i32));
    drain(&mut app);
    assert_eq!(seen_by(&app).into_iter().collect::<Vec<_>>(), [(a, COUNT_SIGNAL)]);
    // And a page that comes back is served normally.
    let mut again = Page::connect(&fx);
    again.step();
    drop(page);
}

#[test]
fn nothing_is_recorded_or_counted_while_no_page_is_attached() {
    let fx = start_devtools();
    let mut app = fx.client();
    let counter = app.new_counter(0);
    storm(&mut app, counter, 300);
    assert!(!fx.bridge.devtools_attached());
    let mut page = Page::connect(&fx);
    // The hub woke when the page came: its counters start there, and the ring has one step (the
    // state the page attached to), not the three hundred commits that came before.
    let stats = page.until("the counters", |m| match m {
        ServerMsg::Stats(j) => Some(serde_json::from_str::<serde_json::Value>(j).unwrap()),
        _ => None,
    });
    assert_eq!(stats["server"]["commits"], 0, "{stats}");
    assert_eq!(stats["server"]["ring_steps"], 1, "{stats}");
    assert_eq!(stats["server"]["steps"], 1, "{stats}");
    // And after it leaves, the hub is idle again and a later page finds the history gone.
    drop(page);
    fx.eventually("the hub to let go", |fx| !fx.bridge.devtools_attached());
    storm(&mut app, counter, 50);
    let mut again = Page::connect(&fx);
    let stats = again.until("the counters", |m| match m {
        ServerMsg::Stats(j) => Some(serde_json::from_str::<serde_json::Value>(j).unwrap()),
        _ => None,
    });
    assert_eq!(stats["server"]["ring_steps"], 1, "{stats}");
    assert_eq!(stats["server"]["commits"], 0, "{stats}");
}
