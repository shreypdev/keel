//! [`FakeWebSocket`]: a scripted WebSocket server for the `WebSocket` port (ADR-047 §8).

use core::fmt;
use core::future::poll_fn;
use core::task::{Poll, Waker};
use std::collections::{BTreeMap, VecDeque};

use parking_lot::Mutex;

use crate::Header;
use crate::ws::{WebSocket, WsError, WsMessage, WsOpened};

/// What [`FakeWebSocket`] knows about one connection (a snapshot).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeWsConnection {
    /// The connection's id (from 1, in connect order).
    pub conn: u32,
    /// The URL the core connected to.
    pub url: String,
    /// The subprotocols it offered.
    pub protocols: Vec<String>,
    /// The headers it sent with the upgrade.
    pub headers: Vec<Header>,
    /// The `(code, reason)` the core closed it with, once it did.
    pub closed_by_core: Option<(u16, String)>,
}

#[derive(Default)]
struct Connection {
    info: Option<FakeWsConnection>,
    inbox: VecDeque<WsMessage>,
    end: Option<WsError>,
    sent: Vec<WsMessage>,
    pulls: Vec<u32>,
    delivered: u64,
    waker: Option<Waker>,
}

#[derive(Default)]
struct State {
    next_id: u32,
    echo: bool,
    refusals: Vec<(String, WsError)>,
    conns: BTreeMap<u32, Connection>,
}

/// A deterministic, in-memory WebSocket server: every connect to a `ws://` or `wss://` URL is
/// accepted (the first offered subprotocol is chosen) unless [`refuse`](FakeWebSocket::refuse)
/// scripted otherwise; the test pushes messages, closes and drops connections, and reads what
/// the core sent and how it pulled.
///
/// Inbound messages wait in the connection's inbox until the core pulls them, so
/// [`delivered`](FakeWebSocket::delivered) against what the core consumed is the backpressure
/// assertion: a core that stopped reading has been handed at most one pull (16) more.
///
/// ```
/// use undra_ports::fakes::{self, FakeWebSocket};
/// use undra_ports::ws::{WsConnection, WsMessage, WsOptions};
/// use undra_runtime::testing::TestRuntime;
///
/// let t = TestRuntime::new();
/// let fakes = fakes::install(&t);
/// fakes.web_socket.echo(true);
/// let ctx = t.ctx();
/// let echoed = t.run_until(async move {
///     let conn = WsConnection::connect(&ctx, "wss://chat.test/", WsOptions::default()).await.unwrap();
///     conn.send_text("hi").await.unwrap();
///     let mut messages = conn.messages();
///     undra_ports::next(&mut messages).await
/// });
/// assert_eq!(echoed, Some(Ok(WsMessage::Text("hi".into()))));
/// ```
#[derive(Default)]
pub struct FakeWebSocket {
    state: Mutex<State>,
}

impl fmt::Debug for FakeWebSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state.lock();
        f.debug_struct("FakeWebSocket")
            .field("connections", &state.conns.len())
            .field("echo", &state.echo)
            .finish()
    }
}

impl FakeWebSocket {
    /// A server that accepts every connection and echoes nothing.
    pub fn new() -> FakeWebSocket {
        FakeWebSocket::default()
    }

    /// Whether every message the core sends comes back to it on the same connection.
    pub fn echo(&self, on: bool) -> &FakeWebSocket {
        self.state.lock().echo = on;
        self
    }

    /// Refuses connects to URLs that start with `url_prefix` with `error`. The first matching
    /// rule wins.
    pub fn refuse(&self, url_prefix: impl Into<String>, error: WsError) -> &FakeWebSocket {
        self.state.lock().refusals.push((url_prefix.into(), error));
        self
    }

    /// Every connection made so far, in connect order.
    pub fn connections(&self) -> Vec<FakeWsConnection> {
        self.state
            .lock()
            .conns
            .values()
            .filter_map(|c| c.info.clone())
            .collect()
    }

    /// The id of the newest connection.
    pub fn last_conn(&self) -> Option<u32> {
        self.state.lock().conns.keys().next_back().copied()
    }

    /// The server sends `message` on `conn`. Ignored once the connection ended.
    pub fn push(&self, conn: u32, message: impl Into<WsMessage>) {
        let waker = {
            let mut state = self.state.lock();
            let Some(c) = state.conns.get_mut(&conn) else {
                return;
            };
            if c.end.is_some() || closed_by_core(c) {
                return;
            }
            c.inbox.push_back(message.into());
            c.waker.take()
        };
        wake(waker);
    }

    /// The server closes `conn` with a close frame: once the inbox is drained the core's stream
    /// ends with `Closed { code, reason }`.
    pub fn close_from_server(&self, conn: u32, code: u16, reason: impl Into<String>) {
        self.end(
            conn,
            WsError::Closed {
                code,
                reason: reason.into(),
            },
        );
    }

    /// The connection drops without a close frame: the stream ends with `Network(message)`.
    pub fn drop_connection(&self, conn: u32, message: impl Into<String>) {
        self.end(conn, WsError::Network(message.into()));
    }

    /// The server breaks the protocol: the stream ends with `Protocol(message)`.
    pub fn break_protocol(&self, conn: u32, message: impl Into<String>) {
        self.end(conn, WsError::Protocol(message.into()));
    }

    fn end(&self, conn: u32, error: WsError) {
        let waker = {
            let mut state = self.state.lock();
            let Some(c) = state.conns.get_mut(&conn) else {
                return;
            };
            if c.end.is_none() {
                c.end = Some(error);
            }
            c.waker.take()
        };
        wake(waker);
    }

    /// What the core sent on `conn`, in order.
    pub fn sent(&self, conn: u32) -> Vec<WsMessage> {
        self.with(conn, |c| c.sent.clone()).unwrap_or_default()
    }

    /// The `max` of every pull (`receive`) the core made on `conn`, in order.
    pub fn pulls(&self, conn: u32) -> Vec<u32> {
        self.with(conn, |c| c.pulls.clone()).unwrap_or_default()
    }

    /// How many messages have been handed to the core on `conn`.
    pub fn delivered(&self, conn: u32) -> u64 {
        self.with(conn, |c| c.delivered).unwrap_or(0)
    }

    /// How many pushed messages are waiting in `conn`'s inbox.
    pub fn waiting(&self, conn: u32) -> usize {
        self.with(conn, |c| c.inbox.len()).unwrap_or(0)
    }

    /// Whether a pull is pending on `conn` (the core is waiting for a message).
    pub fn pulling(&self, conn: u32) -> bool {
        self.with(conn, |c| c.waker.is_some()).unwrap_or(false)
    }

    fn with<R>(&self, conn: u32, f: impl FnOnce(&Connection) -> R) -> Option<R> {
        self.state.lock().conns.get(&conn).map(f)
    }
}

fn closed_by_core(c: &Connection) -> bool {
    c.info
        .as_ref()
        .is_some_and(|info| info.closed_by_core.is_some())
}

fn wake(waker: Option<Waker>) {
    if let Some(waker) = waker {
        waker.wake();
    }
}

fn unknown(conn: u32) -> WsError {
    WsError::Network(format!("no WebSocket connection {conn}"))
}

#[undra_macros::port]
impl WebSocket for FakeWebSocket {
    async fn connect(
        &self,
        url: String,
        protocols: Vec<String>,
        headers: Vec<Header>,
    ) -> Result<WsOpened, WsError> {
        if !(url.starts_with("ws://") || url.starts_with("wss://")) {
            return Err(WsError::Refused {
                status: None,
                message: format!("invalid URL: {url}"),
            });
        }
        let mut state = self.state.lock();
        if let Some((_, error)) = state
            .refusals
            .iter()
            .find(|(prefix, _)| url.starts_with(prefix.as_str()))
        {
            return Err(error.clone());
        }
        state.next_id += 1;
        let conn = state.next_id;
        let protocol = protocols.first().cloned().unwrap_or_default();
        state.conns.insert(
            conn,
            Connection {
                info: Some(FakeWsConnection {
                    conn,
                    url,
                    protocols,
                    headers,
                    closed_by_core: None,
                }),
                ..Connection::default()
            },
        );
        Ok(WsOpened { conn, protocol })
    }

    async fn send(&self, conn: u32, message: WsMessage) -> Result<(), WsError> {
        let waker = {
            let mut state = self.state.lock();
            let echo = state.echo;
            let Some(c) = state.conns.get_mut(&conn) else {
                return Err(unknown(conn));
            };
            if let Some((code, reason)) = c.info.as_ref().and_then(|i| i.closed_by_core.clone()) {
                return Err(WsError::Closed { code, reason });
            }
            if let Some(end) = &c.end {
                return Err(end.clone());
            }
            c.sent.push(message.clone());
            if echo {
                c.inbox.push_back(message);
                c.waker.take()
            } else {
                None
            }
        };
        wake(waker);
        Ok(())
    }

    async fn receive(&self, conn: u32, max: u32) -> Result<Vec<WsMessage>, WsError> {
        {
            let mut state = self.state.lock();
            let Some(c) = state.conns.get_mut(&conn) else {
                return Err(unknown(conn));
            };
            c.pulls.push(max);
        }
        poll_fn(|cx| {
            let mut state = self.state.lock();
            let Some(c) = state.conns.get_mut(&conn) else {
                return Poll::Ready(Err(unknown(conn)));
            };
            if closed_by_core(c) {
                c.waker = None;
                return Poll::Ready(Ok(Vec::new()));
            }
            if !c.inbox.is_empty() {
                let n = c.inbox.len().min(max.max(1) as usize);
                let batch: Vec<WsMessage> = c.inbox.drain(..n).collect();
                c.delivered += batch.len() as u64;
                c.waker = None;
                return Poll::Ready(Ok(batch));
            }
            if let Some(end) = &c.end {
                c.waker = None;
                return Poll::Ready(Err(end.clone()));
            }
            c.waker = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }

    async fn close(&self, conn: u32, code: u16, reason: String) -> Result<(), WsError> {
        let waker = {
            let mut state = self.state.lock();
            let Some(c) = state.conns.get_mut(&conn) else {
                return Err(unknown(conn));
            };
            if let Some(info) = c.info.as_mut() {
                if info.closed_by_core.is_none() {
                    info.closed_by_core = Some((code, reason));
                }
            }
            c.inbox.clear();
            c.waker.take()
        };
        wake(waker);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::testing::block_on;

    #[test]
    fn connects_refuses_and_records() {
        let ws = FakeWebSocket::new();
        ws.refuse(
            "wss://deny.test",
            WsError::Refused {
                status: Some(401),
                message: "no".into(),
            },
        );
        let opened = block_on(ws.connect(
            "wss://ok.test/a".into(),
            vec!["chat".into(), "v2".into()],
            vec![Header::new("a", "b")],
        ))
        .unwrap();
        assert_eq!(
            opened,
            WsOpened {
                conn: 1,
                protocol: "chat".into()
            }
        );
        assert!(matches!(
            block_on(ws.connect("wss://deny.test/x".into(), vec![], vec![])),
            Err(WsError::Refused {
                status: Some(401),
                ..
            })
        ));
        assert!(matches!(
            block_on(ws.connect("https://x".into(), vec![], vec![])),
            Err(WsError::Refused { status: None, .. })
        ));
        let conns = ws.connections();
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].url, "wss://ok.test/a");
        assert_eq!(conns[0].headers, [Header::new("a", "b")]);
        assert_eq!(ws.last_conn(), Some(1));
    }

    #[test]
    fn receive_hands_at_most_max_then_the_end() {
        let ws = FakeWebSocket::new();
        let conn = block_on(ws.connect("ws://x".into(), vec![], vec![]))
            .unwrap()
            .conn;
        for i in 0..5 {
            ws.push(conn, format!("m{i}"));
        }
        ws.close_from_server(conn, 1000, "bye");
        assert_eq!(block_on(ws.receive(conn, 3)).unwrap().len(), 3);
        assert_eq!(block_on(ws.receive(conn, 3)).unwrap().len(), 2);
        assert_eq!(
            block_on(ws.receive(conn, 3)),
            Err(WsError::Closed {
                code: 1000,
                reason: "bye".into()
            })
        );
        assert_eq!(ws.pulls(conn), [3, 3, 3]);
        assert_eq!(ws.delivered(conn), 5);
        ws.push(conn, "late");
        assert_eq!(ws.waiting(conn), 0, "nothing arrives after the end");
        assert!(matches!(
            block_on(ws.send(conn, "x".into())),
            Err(WsError::Closed { .. })
        ));
    }

    #[test]
    fn a_core_close_ends_the_pull_cleanly_and_refuses_sends() {
        let ws = FakeWebSocket::new();
        let conn = block_on(ws.connect("ws://x".into(), vec![], vec![]))
            .unwrap()
            .conn;
        ws.push(conn, "unread");
        block_on(ws.close(conn, 1000, "done".into())).unwrap();
        assert_eq!(block_on(ws.receive(conn, 16)), Ok(Vec::new()));
        assert_eq!(
            block_on(ws.send(conn, "x".into())),
            Err(WsError::Closed {
                code: 1000,
                reason: "done".into()
            })
        );
        assert_eq!(
            ws.connections()[0].closed_by_core,
            Some((1000, "done".into()))
        );
        assert!(matches!(
            block_on(ws.receive(99, 1)),
            Err(WsError::Network(_))
        ));
        assert!(matches!(
            block_on(ws.close(99, 1000, String::new())),
            Err(WsError::Network(_))
        ));
    }
}
