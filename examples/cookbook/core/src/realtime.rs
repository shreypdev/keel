//! Real-time: a WebSocket whose reconnection lives in the core, with server-sent events as the fallback.
//!
//! The platform owns the socket (`URLSessionWebSocketTask`, the Kotlin runtime's client, the
//! browser's `WebSocket`) and never reconnects by itself: which close to retry, how long to wait and
//! what to resend are rules of the app, so they are written once, here, and tested with the fake
//! clock.
//!
//! * [`Live::start`] spawns one task that connects, reads, and when the connection ends sleeps
//!   [`Backoff`] (500 ms doubling to 30 s, with jitter from the `Rng` port) and connects again.
//!   The sleep is the `Timer` port's, so a test moves it with `Fakes::advance`.
//! * A refused upgrade with `401` or `403` stops for good: the credentials are the problem, and
//!   hammering the server does not fix them. A normal close (`1000`) stops too: the server said
//!   goodbye. Anything else is retried.
//! * Every third failed attempt the core tries the event stream instead ([`Link::Fallback`]), for
//!   networks that cut WebSockets. It resumes with the last event id it saw.
//! * `link` says where the connection is, so the UI shows "reconnecting" without knowing anything
//!   about sockets; `messages` is a keyed list that keeps the last hundred.
//! * The reading side is a stream the core pulls, sixteen at a time: a core that stops reading
//!   stops the socket on the platforms that can pause one (ADR-047).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use undra::ports::sse::{self, SseError};
use undra::ports::ws::{WsConnection, WsError, WsMessage, WsOptions};
use undra::ports::{Backoff, CtxPorts, next};
use undra::prelude::*;

/// How many messages the list keeps.
const KEEP: usize = 100;
/// Every this-many failed WebSocket attempts, the core tries the event stream.
const FALLBACK_EVERY: u32 = 3;

/// Where the connection is.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    /// Not started.
    Idle,
    /// The first connection is being opened.
    Connecting,
    /// A WebSocket is open.
    Open,
    /// The last connection ended and the core is waiting to try again.
    Reconnecting {
        /// How many attempts in a row have failed.
        attempt: u32,
    },
    /// The event stream is delivering, because the WebSocket would not connect.
    Fallback,
    /// The core stopped trying, and why.
    Stopped {
        /// What the UI can show.
        reason: String,
    },
}

/// One message from the server.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// Identity in this session; the list is updated by key.
    pub id: u32,
    /// The message's text.
    pub text: String,
}

/// Why a message could not be sent.
#[undra::error]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveError {
    /// There is no open connection; the message was not sent (and is not queued).
    #[error("not connected")]
    NotConnected,
    /// The socket failed while sending.
    #[error("{0}")]
    Ws(#[from] WsError),
}

/// What the store and its task share.
#[derive(Default)]
struct Shared {
    /// The open connection, for `send` and `stop`.
    conn: Mutex<Option<WsConnection>>,
    /// The id of the last event the stream delivered, to resume from.
    last_event_id: Mutex<Option<String>>,
    /// Bumped by `stop`: a task of an older generation ends.
    generation: AtomicU64,
    running: AtomicBool,
    next_id: AtomicU32,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The live-updates store.
#[undra::store(restore = "Self::assemble")]
pub struct Live {
    ctx: WeakCtx,
    shared: Arc<Shared>,
    link: Signal<Link>,
    #[undra(key = "id")]
    messages: Signal<Vec<Message>>,
}

#[undra::api(store)]
impl Live {
    /// A store that is not connected. Call [`start`](Live::start).
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(Link::Idle), Signal::new(vec![]))
    }

    // Used by `new` and, through `restore = ".."`, to rebuild the store from a snapshot. A restored
    // store has no task, so whatever the snapshot said, it is idle.
    fn assemble(ctx: Ctx, link: Signal<Link>, messages: Signal<Vec<Message>>) -> Self {
        if link.get() != Link::Idle {
            link.set(Link::Idle);
        }
        let next = messages.with(|list| list.iter().map(|m| m.id).max().unwrap_or(0));
        let shared = Shared {
            next_id: AtomicU32::new(next + 1),
            ..Shared::default()
        };
        Self {
            ctx: ctx.downgrade(),
            shared: Arc::new(shared),
            link,
            messages,
        }
    }

    /// Connects to the WebSocket at `ws_url` and keeps it connected. `sse_url` is the event stream to
    /// fall back to (empty for none). Does nothing if it is already running.
    pub fn start(&self, ws_url: String, sse_url: String) {
        let Ok(ctx) = self.ctx.upgrade() else { return };
        if self.shared.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let follower = Follower {
            ctx: self.ctx.clone(),
            shared: self.shared.clone(),
            link: self.link.clone(),
            messages: self.messages.clone(),
            ws_url,
            sse_url,
            mine: self.shared.generation.load(Ordering::SeqCst),
        };
        self.link.set(Link::Connecting);
        ctx.spawn(follower.run());
    }

    /// Closes the connection and stops reconnecting.
    pub fn stop(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        self.shared.running.store(false, Ordering::SeqCst);
        let conn = lock(&self.shared.conn).take();
        if let (Some(conn), Ok(ctx)) = (conn, self.ctx.upgrade()) {
            ctx.spawn(async move {
                let _ = conn.close(1000, "").await;
            });
        }
        if self.link.get() != Link::Idle {
            self.link.set(Link::Idle);
        }
    }

    /// Sends a text message on the open connection. Not queued: a message sent while the link is
    /// down fails with [`LiveError::NotConnected`], and the UI decides whether to try again.
    pub async fn send(&self, text: String) -> Result<(), LiveError> {
        let conn = lock(&self.shared.conn).clone();
        match conn {
            Some(conn) => Ok(conn.send_text(text).await?),
            None => Err(LiveError::NotConnected),
        }
    }
}

/// The task `start` spawns. It holds a `WeakCtx`, never the store or a `Ctx` across a wait
/// (ADR-034): when the store goes away or the runtime shuts down, the next sleep ends it.
struct Follower {
    ctx: WeakCtx,
    shared: Arc<Shared>,
    link: Signal<Link>,
    messages: Signal<Vec<Message>>,
    ws_url: String,
    sse_url: String,
    mine: u64,
}

impl Follower {
    fn stopped(&self) -> bool {
        self.shared.generation.load(Ordering::SeqCst) != self.mine
    }

    fn give_up(&self, reason: &str) {
        lock(&self.shared.conn).take();
        self.shared.running.store(false, Ordering::SeqCst);
        self.link.set(Link::Stopped {
            reason: reason.to_owned(),
        });
    }

    async fn run(self) {
        let backoff = Backoff::default();
        let mut attempt: u32 = 0;
        loop {
            if self.stopped() {
                return;
            }
            let Ok(ctx) = self.ctx.upgrade() else { return };
            match WsConnection::connect(&ctx, &self.ws_url, WsOptions::default()).await {
                Ok(conn) => {
                    drop(ctx);
                    *lock(&self.shared.conn) = Some(conn.clone());
                    self.link.set(Link::Open);
                    let mut inbound = conn.messages();
                    let mut first = true;
                    while let Some(item) = next(&mut inbound).await {
                        if self.stopped() {
                            return;
                        }
                        match item {
                            Ok(WsMessage::Text(text)) => {
                                if first {
                                    // A server that accepts and drops at once must keep backing off:
                                    // the count resets when a message arrives, not when the socket opens.
                                    first = false;
                                    attempt = 0;
                                }
                                self.push(text);
                            }
                            Ok(WsMessage::Binary(_)) => {}
                            // The server said goodbye on purpose: do not hammer it.
                            Err(WsError::Closed { code: 1000, .. }) => {
                                return self.give_up("the server closed the connection");
                            }
                            Err(_) => break,
                        }
                    }
                    lock(&self.shared.conn).take();
                }
                // The credentials are the problem, not the network: another attempt fails the same way.
                Err(WsError::Refused {
                    status: Some(401 | 403),
                    ..
                }) => return self.give_up("the server refused the credentials"),
                Err(_) => drop(ctx),
            }
            if self.stopped() {
                return;
            }
            attempt += 1;
            self.link.set(Link::Reconnecting { attempt });

            if attempt % FALLBACK_EVERY == 0 && !self.sse_url.is_empty() {
                self.fall_back().await;
                if self.stopped() {
                    return;
                }
                self.link.set(Link::Reconnecting { attempt });
            }

            let Ok(ctx) = self.ctx.upgrade() else { return };
            let delay = backoff.delay(attempt, &*ctx.rng());
            drop(ctx);
            if self.ctx.sleep(delay).await.is_err() {
                return;
            }
        }
    }

    /// Reads the event stream until it ends or fails, resuming from the last event id.
    async fn fall_back(&self) {
        let Ok(ctx) = self.ctx.upgrade() else { return };
        let resume = lock(&self.shared.last_event_id).clone();
        let mut events = sse::subscribe(&ctx, &self.sse_url, vec![], resume);
        drop(ctx);
        self.link.set(Link::Fallback);
        while let Some(item) = next(&mut events).await {
            if self.stopped() {
                return;
            }
            match item {
                Ok(event) => {
                    if let Some(id) = event.id {
                        *lock(&self.shared.last_event_id) = Some(id);
                    }
                    self.push(event.data);
                }
                Err(SseError::Refused { .. }) | Err(_) => return,
            }
        }
    }

    /// Appends one message and drops the oldest past [`KEEP`]: one transaction.
    fn push(&self, text: String) {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        txn(|| {
            self.messages.push(Message { id, text });
            if self.messages.with(Vec::len) > KEEP {
                self.messages.remove(0);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use undra::ports::sse::SseEvent;
    use undra::ports::ws::WsMessage;

    use super::*;
    use crate::net::testing::App;

    const WS: &str = "wss://live.test/room";
    const SSE: &str = "https://live.test/stream";

    fn started(app: &App) -> Live {
        let live = Live::new(app.ctx());
        live.start(WS.into(), SSE.into());
        app.t.run_pending();
        live
    }

    fn texts(live: &Live) -> Vec<String> {
        live.messages
            .with(|m| m.iter().map(|m| m.text.clone()).collect())
    }

    #[test]
    fn it_connects_and_messages_arrive() {
        let app = App::new();
        let live = started(&app);
        assert_eq!(live.link.get(), Link::Open);
        let conn = app.fakes.web_socket.last_conn().expect("connected");
        app.fakes.web_socket.push(conn, "hello");
        app.fakes.web_socket.push(conn, "world");
        app.t.run_pending();
        assert_eq!(texts(&live), ["hello", "world"]);
    }

    #[test]
    fn a_dropped_connection_is_reopened_after_a_backoff() {
        let app = App::new();
        let live = started(&app);
        let first = app.fakes.web_socket.last_conn().unwrap();
        app.fakes.web_socket.push(first, "one");
        app.t.run_pending();
        app.fakes
            .web_socket
            .drop_connection(first, "connection reset");
        app.t.run_pending();
        assert_eq!(live.link.get(), Link::Reconnecting { attempt: 1 });
        assert_eq!(
            app.fakes.web_socket.connections().len(),
            1,
            "it waits before trying again"
        );

        app.advance(500); // the first delay is at most 500 ms
        assert_eq!(live.link.get(), Link::Open);
        let second = app.fakes.web_socket.last_conn().unwrap();
        assert_ne!(first, second);
        app.fakes.web_socket.push(second, "two");
        app.t.run_pending();
        assert_eq!(texts(&live), ["one", "two"]);
    }

    #[test]
    fn the_wait_doubles_while_the_server_stays_down() {
        let app = App::new();
        app.fakes
            .web_socket
            .refuse("wss://live.test", WsError::Network("down".into()));
        let live = Live::new(app.ctx());
        live.start(WS.into(), String::new());
        app.t.run_pending();
        assert_eq!(live.link.get(), Link::Reconnecting { attempt: 1 });
        // Attempt n waits at most 500 ms * 2^(n-1) (and at least half of it): after 0.5 s the
        // second attempt has failed, after another 1 s the third, after another 2 s the fourth.
        app.advance(500);
        assert_eq!(live.link.get(), Link::Reconnecting { attempt: 2 });
        app.advance(1_000);
        assert_eq!(live.link.get(), Link::Reconnecting { attempt: 3 });
        app.advance(2_000);
        assert_eq!(live.link.get(), Link::Reconnecting { attempt: 4 });
    }

    #[test]
    fn refused_credentials_stop_for_good() {
        let app = App::new();
        app.fakes.web_socket.refuse(
            "wss://live.test",
            WsError::Refused {
                status: Some(401),
                message: "no".into(),
            },
        );
        let live = started(&app);
        assert_eq!(
            live.link.get(),
            Link::Stopped {
                reason: "the server refused the credentials".into()
            }
        );
        app.advance(120_000);
        assert!(app.fakes.web_socket.connections().is_empty());
        assert!(
            matches!(live.link.get(), Link::Stopped { .. }),
            "and it does not try again"
        );
    }

    #[test]
    fn a_normal_close_is_a_goodbye_not_a_failure() {
        let app = App::new();
        let live = started(&app);
        let conn = app.fakes.web_socket.last_conn().unwrap();
        app.fakes.web_socket.close_from_server(conn, 1000, "bye");
        app.t.run_pending();
        assert_eq!(
            live.link.get(),
            Link::Stopped {
                reason: "the server closed the connection".into()
            }
        );
        app.advance(60_000);
        assert_eq!(app.fakes.web_socket.connections().len(), 1);
    }

    #[test]
    fn a_network_that_cuts_websockets_falls_back_to_the_event_stream_and_resumes_by_id() {
        let app = App::new();
        app.fakes
            .web_socket
            .refuse("wss://live.test", WsError::Network("blocked".into()));
        let live = started(&app); // attempt 1 failed
        app.advance(500); // 2
        app.advance(1_000); // 3: the event stream
        assert_eq!(live.link.get(), Link::Fallback);
        let stream = app.fakes.sse.last_stream().expect("subscribed");
        assert_eq!(app.fakes.sse.streams()[0].last_event_id, None);
        app.fakes
            .sse
            .push(stream, SseEvent::message("from sse").with_id("7"));
        app.t.run_pending();
        assert_eq!(texts(&live), ["from sse"]);

        // The stream ends; the core waits, tries the WebSocket twice more, then resumes the stream
        // from the last event it saw.
        app.fakes.sse.end(stream);
        app.advance(120_000);
        let streams = app.fakes.sse.streams();
        assert!(streams.len() >= 2, "{streams:?}");
        assert_eq!(streams[1].last_event_id.as_deref(), Some("7"));
    }

    #[test]
    fn sending_needs_an_open_connection() {
        let app = App::new();
        let live = Live::new(app.ctx());
        assert_eq!(
            app.run(live.send("early".into())),
            Err(LiveError::NotConnected)
        );
        live.start(WS.into(), String::new());
        app.t.run_pending();
        assert_eq!(app.run(live.send("hi".into())), Ok(()));
        let conn = app.fakes.web_socket.last_conn().unwrap();
        assert_eq!(
            app.fakes.web_socket.sent(conn),
            [WsMessage::Text("hi".into())]
        );
    }

    #[test]
    fn stop_closes_the_connection_and_does_not_reconnect() {
        let app = App::new();
        let live = started(&app);
        let conn = app.fakes.web_socket.last_conn().unwrap();
        live.stop();
        app.t.run_pending();
        assert_eq!(live.link.get(), Link::Idle);
        assert_eq!(
            app.fakes.web_socket.connections()[0].closed_by_core,
            Some((1000, String::new()))
        );
        app.advance(60_000);
        assert_eq!(app.fakes.web_socket.connections().len(), 1);
        assert_eq!(app.fakes.web_socket.last_conn(), Some(conn));
    }

    #[test]
    fn the_list_keeps_the_last_hundred() {
        let app = App::new();
        let live = started(&app);
        let conn = app.fakes.web_socket.last_conn().unwrap();
        for n in 0..130 {
            app.fakes.web_socket.push(conn, format!("m{n}"));
        }
        app.t.run_pending();
        let kept = texts(&live);
        assert_eq!(kept.len(), KEEP);
        assert_eq!(kept.first().map(String::as_str), Some("m30"));
        assert_eq!(kept.last().map(String::as_str), Some("m129"));
    }
}
