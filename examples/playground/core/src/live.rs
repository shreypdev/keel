//! Real-time through the opt-in ports (ADR-047): a WebSocket connection the UI drives through
//! [`Live`], a one-shot echo ([`ws_echo`]) and a server-sent event reader ([`sse_follow`]).
//!
//! The core reaches the network only through the `WebSocket` and `Sse` ports, so the contract
//! runners (S23, S24) serve them with a scripted in-process server written to each runtime's
//! public adapter interface, the playground apps with the platform's own client, and the tests
//! below with `undra::ports::fakes`. Inbound messages are pulled with the core's credit: a
//! [`Live`] that is not asked to [`read`](Live::read) leaves the server's messages on the
//! platform, at most one pull (16) ahead.

use std::sync::{Mutex, MutexGuard, PoisonError};

use undra::ports::Header;
use undra::ports::sse::{self, SseError, SseEvent};
use undra::ports::ws::{WsConnection, WsError, WsMessage, WsMessages, WsOptions};
use undra::prelude::*;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Connects to `url`, sends `messages` in order, reads as many back and closes (1000).
///
/// Against an echo server the replies are the messages; any end of the connection before that is
/// the error (a refusal, `Closed` with the server's code, `Network`).
#[undra::api]
pub async fn ws_echo(
    ctx: &Ctx,
    url: String,
    messages: Vec<WsMessage>,
) -> Result<Vec<WsMessage>, WsError> {
    let conn = WsConnection::connect(ctx, &url, WsOptions::default()).await?;
    let count = messages.len();
    for message in messages {
        conn.send(message).await?;
    }
    let mut inbound = conn.messages();
    let mut replies = Vec::with_capacity(count);
    while replies.len() < count {
        match undra::ports::next(&mut inbound).await {
            Some(Ok(message)) => replies.push(message),
            Some(Err(error)) => return Err(error),
            None => break,
        }
    }
    conn.close(1000, "done").await?;
    Ok(replies)
}

/// What [`sse_follow`] read.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseFollow {
    /// The events, in order.
    pub events: Vec<SseEvent>,
    /// Whether the server ended the stream (`SseError::Ended`) before `max` events.
    pub ended: bool,
}

/// Subscribes to `url` (resuming after `last_event_id`) and reads up to `max` events, or until
/// the server ends the stream. Reconnecting is the caller's: call again with the last id read.
#[undra::api]
pub async fn sse_follow(
    ctx: &Ctx,
    url: String,
    last_event_id: Option<String>,
    max: u32,
) -> Result<SseFollow, SseError> {
    let mut events = sse::subscribe(ctx, &url, Vec::new(), last_event_id);
    let mut read = Vec::new();
    while read.len() < max as usize {
        match undra::ports::next(&mut events).await {
            Some(Ok(event)) => read.push(event),
            Some(Err(SseError::Ended)) => {
                return Ok(SseFollow {
                    events: read,
                    ended: true,
                });
            }
            Some(Err(error)) => return Err(error),
            None => break,
        }
    }
    events.close().await?;
    Ok(SseFollow {
        events: read,
        ended: false,
    })
}

#[derive(Default)]
struct Line {
    /// Which connection this is: a `read` that started on an older one does not put its stream
    /// back over a newer one's.
    generation: u64,
    conn: Option<WsConnection>,
    inbound: Option<WsMessages>,
}

/// One WebSocket connection the platform drives step by step: connect, send, read a given number
/// of messages, close or abandon. Between reads nothing is pulled beyond the stream's credit.
pub struct Live {
    /// Weak (ADR-034): the object table holds the object, so a `Ctx` here would pin the runtime.
    ctx: WeakCtx,
    line: Mutex<Line>,
}

#[undra::api]
impl Live {
    /// No connection yet.
    pub fn new(ctx: Ctx) -> Self {
        Live {
            ctx: ctx.downgrade(),
            line: Mutex::new(Line::default()),
        }
    }

    /// Connects to `url`, offering `protocols` and sending `headers`; a connection already open is
    /// closed first (1000). Returns the subprotocol the server chose (`""` for none).
    pub async fn connect(
        &self,
        url: String,
        protocols: Vec<String>,
        headers: Vec<Header>,
    ) -> Result<String, WsError> {
        let previous = {
            let mut line = lock(&self.line);
            let generation = line.generation + 1;
            core::mem::replace(
                &mut *line,
                Line {
                    generation,
                    ..Line::default()
                },
            )
        };
        if let Some(conn) = previous.conn {
            drop(previous.inbound);
            let _ = conn.close(1000, "replaced").await;
        }
        let Ok(ctx) = self.ctx.upgrade() else {
            return Err(WsError::Network("the runtime is gone".to_owned()));
        };
        let conn = WsConnection::connect(&ctx, &url, WsOptions { protocols, headers }).await?;
        let protocol = conn.protocol().to_owned();
        let inbound = conn.messages();
        let mut line = lock(&self.line);
        line.generation += 1;
        line.conn = Some(conn);
        line.inbound = Some(inbound);
        Ok(protocol)
    }

    /// Sends `message` on the open connection.
    pub async fn send(&self, message: WsMessage) -> Result<(), WsError> {
        let conn = lock(&self.line).conn.clone().ok_or_else(not_connected)?;
        conn.send(message).await
    }

    /// Reads exactly `count` messages. The connection's end before that is the error (its
    /// close code, a network failure); a stream that ended cleanly (after [`disconnect`](Live::disconnect))
    /// returns the messages it had.
    pub async fn read(&self, count: u32) -> Result<Vec<WsMessage>, WsError> {
        let (generation, mut inbound) = {
            let mut line = lock(&self.line);
            let inbound = line.inbound.take().ok_or_else(not_connected)?;
            (line.generation, inbound)
        };
        let mut read = Vec::with_capacity(count as usize);
        let mut outcome = Ok(());
        while read.len() < count as usize {
            match undra::ports::next(&mut inbound).await {
                Some(Ok(message)) => read.push(message),
                Some(Err(error)) => {
                    outcome = Err(error);
                    break;
                }
                None => break,
            }
        }
        let mut line = lock(&self.line);
        if line.generation == generation {
            line.inbound = Some(inbound);
        }
        drop(line);
        outcome.map(|()| read)
    }

    /// How many pulls the inbound stream has made (each asks the platform for up to 16).
    pub fn pulls(&self) -> u64 {
        lock(&self.line)
            .inbound
            .as_ref()
            .map_or(0, WsMessages::pulls)
    }

    /// Closes the connection with `code` and `reason` (`close` is every object's own: it releases
    /// the handle).
    pub async fn disconnect(&self, code: u16, reason: String) -> Result<(), WsError> {
        let conn = lock(&self.line).conn.clone().ok_or_else(not_connected)?;
        conn.close(code, reason).await
    }

    /// Forgets the connection without closing it: the port closes it going away (1001).
    pub fn abandon(&self) {
        let line = core::mem::take(&mut *lock(&self.line));
        drop(line);
    }
}

fn not_connected() -> WsError {
    WsError::Network("not connected: call connect first".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use undra::ports::fakes;
    use undra::runtime::testing::TestRuntime;

    #[test]
    fn echo_sends_and_reads_back() {
        let t = TestRuntime::new();
        let fakes = fakes::install(&t);
        fakes.web_socket.echo(true);
        let ctx = t.ctx();
        let replies = t.run_until(async move {
            ws_echo(
                &ctx,
                "ws://echo.test".into(),
                vec![
                    WsMessage::Text("a".into()),
                    WsMessage::Binary(vec![1].into()),
                ],
            )
            .await
        });
        assert_eq!(
            replies,
            Ok(vec![
                WsMessage::Text("a".into()),
                WsMessage::Binary(vec![1].into())
            ])
        );
        assert_eq!(
            fakes.web_socket.connections()[0].closed_by_core,
            Some((1000, "done".into()))
        );
    }

    #[test]
    fn live_reads_on_demand_and_reports_the_end() {
        let t = TestRuntime::new();
        let fakes = fakes::install(&t);
        let live = std::sync::Arc::new(Live::new(t.ctx()));
        let l = live.clone();
        let protocol = t.run_until(async move {
            l.connect("ws://x.test".into(), vec!["v1".into()], vec![])
                .await
        });
        assert_eq!(protocol.as_deref(), Ok("v1"));
        let conn = fakes.web_socket.last_conn().unwrap();
        for i in 0..100 {
            fakes.web_socket.push(conn, format!("{i}"));
        }
        let l = live.clone();
        let five = t.run_until(async move { l.read(5).await }).unwrap();
        assert_eq!(five.len(), 5);
        assert!(fakes.web_socket.delivered(conn) <= 5 + 16 + 8);
        fakes.web_socket.close_from_server(conn, 4000, "bye");
        let l = live.clone();
        let rest = t.run_until(async move { l.read(95).await }).unwrap();
        assert_eq!(rest.last(), Some(&WsMessage::Text("99".into())));
        let l = live.clone();
        let end = t.run_until(async move { l.read(1).await });
        assert_eq!(
            end,
            Err(WsError::Closed {
                code: 4000,
                reason: "bye".into()
            })
        );
        assert!(live.pulls() >= 7);
        live.abandon();
        t.run_pending();
        assert_eq!(
            fakes.web_socket.connections()[0].closed_by_core,
            Some((1001, String::new()))
        );
    }

    #[test]
    fn a_read_on_an_old_connection_does_not_replace_the_new_one() {
        let t = TestRuntime::new();
        let fakes = fakes::install(&t);
        let live = std::sync::Arc::new(Live::new(t.ctx()));
        let l = live.clone();
        t.run_until(async move { l.connect("ws://a.test".into(), vec![], vec![]).await })
            .unwrap();
        let first = fakes.web_socket.last_conn().unwrap();
        // A read waits on the first connection while the platform connects again.
        let l = live.clone();
        let reading = t.ctx().spawn(async move {
            let _ = l.read(1).await;
        });
        t.run_pending();
        let l = live.clone();
        t.run_until(async move { l.connect("ws://b.test".into(), vec![], vec![]).await })
            .unwrap();
        let second = fakes.web_socket.last_conn().unwrap();
        assert_ne!(first, second);
        fakes.web_socket.push(first, "late");
        t.run_pending();
        let _ = reading;
        fakes.web_socket.push(second, "fresh");
        let l = live.clone();
        let read = t.run_until(async move { l.read(1).await });
        assert_eq!(read, Ok(vec![WsMessage::Text("fresh".into())]));
    }

    #[test]
    fn sse_follow_reads_until_the_server_ends() {
        let t = TestRuntime::new();
        let fakes = fakes::install(&t);
        let ctx = t.ctx();
        let task = t.ctx().spawn(async move {
            let _ = sse_follow(&ctx, "https://feed.test".into(), Some("7".into()), 10).await;
        });
        let _ = task;
        t.run_pending();
        let stream = fakes.sse.last_stream().unwrap();
        assert_eq!(fakes.sse.streams()[0].last_event_id.as_deref(), Some("7"));
        fakes.sse.push(stream, SseEvent::message("x").with_id("8"));
        fakes.sse.end(stream);
        t.run_pending();
        assert_eq!(fakes.sse.delivered(stream), 1);
    }
}
