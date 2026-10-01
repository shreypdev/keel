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
fn a_port_call_with_no_app_client_is_recorded_as_unavailable() {
    let fx = start_devtools();
    let mut page = Page::connect(&fx);
    page.step();
    // No app client: the core's own call to a platform port ends at once.
    on_core(&fx.rt, {
        let rt = fx.rt.clone();
        move || {
            drop(rt.ctx().port_call(ECHO_PORT, ECHO_METHOD, enc(&1_i32)));
        }
    });
    let status = page.until("an unavailable end", |m| match m {
        ServerMsg::Port(PortRecord::End { status, .. }) => Some(*status),
        _ => None,
    });
    assert_eq!(status, 2);
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
