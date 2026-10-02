//! Presence: who else is in the notebook right now, and which note they are looking at.
//!
//! Feature `presence`, because it needs the opt-in `WebSocket` port (ADR-047). The socket belongs to
//! the platform; reconnecting is the core's business, so it is written once here (the cookbook's
//! real-time page has the longer version, with the event-stream fallback):
//!
//! * the task connects, reads the server's `presence` messages into `members`, and when the
//!   connection ends sleeps [`Backoff`] (500 ms doubling to 30 s, jittered from the `Rng` port) and
//!   connects again, on the `Timer` port, so a test moves it with the fake clock;
//! * a refused upgrade (`401`/`403`) or a normal close stops it: another attempt would fail the same way;
//! * the credential goes in the URL, because a browser cannot send headers on a WebSocket.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::Deserialize;
use undra::ports::ws::{WsConnection, WsError, WsMessage, WsOptions};
use undra::ports::{Backoff, CtxPorts, next};
use undra::prelude::*;

/// Where the presence connection is.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    /// Not started.
    Idle,
    /// A WebSocket is open.
    Open,
    /// The last connection ended; the core is waiting to try again.
    Reconnecting {
        /// How many attempts in a row have failed.
        attempt: u32,
    },
    /// The core stopped trying, and why.
    Stopped {
        /// What the UI can show.
        reason: String,
    },
}

/// A member who is in the notebook.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Member {
    /// Their name; the list is updated by key.
    pub name: String,
    /// The note they are looking at, if any.
    #[serde(default)]
    pub viewing: Option<u32>,
}

#[derive(Deserialize)]
struct Message {
    members: Vec<Member>,
}

#[derive(Default)]
struct Shared {
    conn: Mutex<Option<WsConnection>>,
    generation: AtomicU64,
    running: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The presence store.
#[undra::store(restore = "Self::assemble")]
pub struct Presence {
    ctx: WeakCtx,
    shared: Arc<Shared>,
    link: Signal<Link>,
    #[undra(key = "name")]
    members: Signal<Vec<Member>>,
}

#[undra::api(store)]
impl Presence {
    /// A store that is not connected. Call `join`.
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(Link::Idle), Signal::new(vec![]))
    }

    // A restored store has no task: whatever the snapshot said, it is idle.
    fn assemble(ctx: Ctx, link: Signal<Link>, members: Signal<Vec<Member>>) -> Self {
        if link.get() != Link::Idle {
            link.set(Link::Idle);
        }
        Self {
            ctx: ctx.downgrade(),
            shared: Arc::new(Shared::default()),
            link,
            members,
        }
    }

    /// Joins the room at `url` (`wss://..`) with the member's token and keeps the connection.
    pub fn join(&self, url: String, token: String) {
        let Ok(ctx) = self.ctx.upgrade() else { return };
        if self.shared.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let task = Follower {
            ctx: self.ctx.clone(),
            shared: self.shared.clone(),
            link: self.link.clone(),
            members: self.members.clone(),
            url: format!("{url}?token={token}"),
            mine: self.shared.generation.load(Ordering::SeqCst),
        };
        ctx.spawn(task.run());
    }

    /// Leaves the room and stops reconnecting.
    pub fn leave(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        self.shared.running.store(false, Ordering::SeqCst);
        let conn = lock(&self.shared.conn).take();
        if let (Some(conn), Ok(ctx)) = (conn, self.ctx.upgrade()) {
            ctx.spawn(async move {
                let _ = conn.close(1000, "").await;
            });
        }
        self.link.set(Link::Idle);
        self.members.set(vec![]);
    }

    /// Tells the others which note this member is looking at (none: `None`). Dropped, not queued,
    /// while the link is down: presence is a fact about now.
    pub async fn set_viewing(&self, note: Option<u32>) {
        let conn = lock(&self.shared.conn).clone();
        if let Some(conn) = conn {
            let _ = conn
                .send_text(serde_json::json!({ "type": "viewing", "note": note }).to_string())
                .await;
        }
    }
}

struct Follower {
    ctx: WeakCtx,
    shared: Arc<Shared>,
    link: Signal<Link>,
    members: Signal<Vec<Member>>,
    url: String,
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

    // docs:begin fieldbook-presence
    async fn run(self) {
        let backoff = Backoff::default();
        let mut attempt: u32 = 0;
        loop {
            if self.stopped() {
                return;
            }
            let Ok(ctx) = self.ctx.upgrade() else { return };
            match WsConnection::connect(&ctx, &self.url, WsOptions::default()).await {
                Ok(conn) => {
                    drop(ctx);
                    *lock(&self.shared.conn) = Some(conn.clone());
                    self.link.set(Link::Open);
                    let mut inbound = conn.messages();
                    while let Some(item) = next(&mut inbound).await {
                        if self.stopped() {
                            return;
                        }
                        match item {
                            Ok(WsMessage::Text(text)) => {
                                attempt = 0; // it is talking: the next failure starts the backoff again
                                if let Ok(message) = serde_json::from_str::<Message>(&text) {
                                    self.members.set(message.members);
                                }
                            }
                            Ok(WsMessage::Binary(_)) => {}
                            Err(WsError::Closed { code: 1000, .. }) => {
                                return self.give_up("the server closed the connection");
                            }
                            Err(_) => break,
                        }
                    }
                    lock(&self.shared.conn).take();
                }
                Err(WsError::Refused {
                    status: Some(401 | 403),
                    ..
                }) => return self.give_up("sign in again"),
                Err(_) => drop(ctx),
            }
            if self.stopped() {
                return;
            }
            attempt += 1;
            // Nobody can be seen while the link is down: show nobody rather than stale faces.
            self.members.set(vec![]);
            self.link.set(Link::Reconnecting { attempt });
            let Ok(ctx) = self.ctx.upgrade() else { return };
            let delay = backoff.delay(attempt, &*ctx.rng());
            drop(ctx);
            if self.ctx.sleep(delay).await.is_err() {
                return;
            }
        }
    }
    // docs:end
}

#[cfg(test)]
mod tests {
    use undra::ports::ws::WsMessage;

    use super::*;
    use crate::net::testing::App;

    fn joined(app: &App) -> Presence {
        let room = Presence::new(app.ctx());
        room.join("wss://fieldbook.test/presence".into(), "t0k".into());
        app.t.run_pending();
        room
    }

    fn names(room: &Presence) -> Vec<String> {
        room.members
            .with(|m| m.iter().map(|m| m.name.clone()).collect())
    }

    #[test]
    fn it_joins_with_the_token_in_the_url_and_shows_who_is_there() {
        let app = App::new();
        let room = joined(&app);
        assert_eq!(room.link.get(), Link::Open);
        let conn = app.fakes.web_socket.last_conn().unwrap();
        assert_eq!(
            app.fakes.web_socket.connections()[0].url,
            "wss://fieldbook.test/presence?token=t0k"
        );
        app.fakes.web_socket.push(
            conn,
            r#"{"type":"presence","members":[{"name":"Ada","viewing":3},{"name":"Grace"}]}"#,
        );
        app.t.run_pending();
        assert_eq!(names(&room), ["Ada", "Grace"]);
        assert_eq!(room.members.get()[0].viewing, Some(3));
    }

    #[test]
    fn a_dropped_connection_shows_nobody_and_is_reopened_after_a_backoff() {
        let app = App::new();
        let room = joined(&app);
        let first = app.fakes.web_socket.last_conn().unwrap();
        app.fakes
            .web_socket
            .push(first, r#"{"members":[{"name":"Ada"}]}"#);
        app.t.run_pending();
        app.fakes.web_socket.drop_connection(first, "reset");
        app.t.run_pending();
        assert_eq!(room.link.get(), Link::Reconnecting { attempt: 1 });
        assert!(names(&room).is_empty(), "no stale faces");
        app.advance(500);
        assert_eq!(room.link.get(), Link::Open);
        assert_ne!(app.fakes.web_socket.last_conn(), Some(first));
    }

    #[test]
    fn refused_credentials_stop_and_the_viewing_note_is_sent_only_while_connected() {
        let app = App::new();
        app.fakes.web_socket.refuse(
            "wss://fieldbook.test",
            WsError::Refused {
                status: Some(401),
                message: "no".into(),
            },
        );
        let room = joined(&app);
        assert_eq!(
            room.link.get(),
            Link::Stopped {
                reason: "sign in again".into()
            }
        );
        app.run(room.set_viewing(Some(1))); // nothing to send on, and no panic

        let app = App::new();
        let room = joined(&app);
        app.run(room.set_viewing(Some(4)));
        let conn = app.fakes.web_socket.last_conn().unwrap();
        assert_eq!(
            app.fakes.web_socket.sent(conn),
            [WsMessage::Text(r#"{"note":4,"type":"viewing"}"#.into())]
        );
    }

    #[test]
    fn leaving_closes_the_socket_and_clears_the_room() {
        let app = App::new();
        let room = joined(&app);
        let conn = app.fakes.web_socket.last_conn().unwrap();
        app.fakes
            .web_socket
            .push(conn, r#"{"members":[{"name":"Ada"}]}"#);
        app.t.run_pending();
        room.leave();
        app.t.run_pending();
        assert_eq!((room.link.get(), names(&room)), (Link::Idle, vec![]));
        assert_eq!(
            app.fakes.web_socket.connections()[0].closed_by_core,
            Some((1000, String::new()))
        );
        app.advance(60_000);
        assert_eq!(
            app.fakes.web_socket.connections().len(),
            1,
            "it does not come back"
        );
    }
}
