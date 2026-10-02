//! The `WebSocket` port (ADR-047; feature `websocket`): a connection the platform opens, messages
//! out with [`WsConnection::send`], and messages in as a [`Stream`] whose pull is the core's
//! credit.
//!
//! # Wire contract
//!
//! | Type | Wire form |
//! |---|---|
//! | [`WsOpened`] | `conn u32, protocol String` |
//! | [`WsMessage`] | `u16` index: `Text(String)` 0, `Binary(Bytes)` 1 |
//! | [`WsError`] | `u16` index: `Refused { status: Option<u16>, message: String }` 0, `Network(String)` 1, `Protocol(String)` 2, `Closed { code: u16, reason: String }` 3 |
//!
//! # How inbound backpressure works
//!
//! A port call cannot return a stream (SPEC 3.6), so the inbound side is pulled:
//! `receive(conn, max)` answers as soon as at least one message is buffered on the platform,
//! with at most `max` of them. `Ok([])` means the core closed the connection; an `Err` ends the
//! stream with the reason. The adapter reads ahead at most `max` messages of the latest pull
//! (16 before the first), so a core that stops reading stops the socket where the platform can
//! pause it (ADR-047 §3). [`WsMessages`] keeps one pull of [`CREDIT`] in flight and asks for the
//! next while it still holds fewer than [`LOW_WATER`], the numbers of SPEC 3.7.
//!
//! # Reconnecting
//!
//! The port never reconnects by itself: the core decides, with [`Backoff`](crate::Backoff) and
//! `WeakCtx::sleep` (ADR-034), so the policy is deterministic and testable with `FakeClock`.
//!
//! ```no_run
//! use undra_ports::ws::{WsConnection, WsError, WsMessage, WsOptions};
//! use undra_ports::{Backoff, CtxPorts};
//! use undra_runtime::WeakCtx;
//!
//! async fn follow(weak: WeakCtx, url: String) {
//!     let backoff = Backoff::default();
//!     let mut attempt = 0;
//!     loop {
//!         let Ok(ctx) = weak.upgrade() else { return };
//!         match WsConnection::connect(&ctx, &url, WsOptions::default()).await {
//!             Ok(conn) => {
//!                 attempt = 0;
//!                 let mut messages = conn.messages();
//!                 while let Some(next) = undra_ports::next(&mut messages).await {
//!                     match next {
//!                         Ok(WsMessage::Text(text)) => { let _ = text; /* apply it */ }
//!                         Ok(WsMessage::Binary(_)) => {}
//!                         // The server said goodbye on purpose: do not hammer it.
//!                         Err(WsError::Closed { code: 1000, .. }) => return,
//!                         Err(_) => break,
//!                     }
//!                 }
//!             }
//!             Err(WsError::Refused { status: Some(401 | 403), .. }) => return,
//!             Err(_) => {}
//!         }
//!         attempt += 1;
//!         let delay = backoff.delay(attempt, &*ctx.rng());
//!         drop(ctx);
//!         if weak.sleep(delay).await.is_err() {
//!             return;
//!         }
//!     }
//! }
//! ```

use core::fmt;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_core::Stream;
use undra_runtime::{Ctx, PortError, WeakCtx};
use undra_wire::{Bytes, Decode};

use crate::records::Header;

/// How many messages one pull asks for (SPEC 3.7's initial grant).
pub const CREDIT: u32 = 16;

/// [`WsMessages`] issues the next pull while it holds fewer messages than this (SPEC 3.7's
/// top-up threshold).
pub const LOW_WATER: usize = 8;

/// What `connect` answers: the connection's id and the subprotocol the server chose.
#[undra_macros::api]
#[undra(crate = "crate::root")]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WsOpened {
    /// The connection's id, chosen by the adapter; never reused by it.
    pub conn: u32,
    /// The subprotocol the server selected, or `""` when none was negotiated.
    pub protocol: String,
}

/// One WebSocket message. Control frames (ping, pong, close) never surface as messages.
#[undra_macros::api]
#[undra(crate = "crate::root")]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WsMessage {
    /// A text message (valid UTF-8 by RFC 6455).
    Text(String),
    /// A binary message.
    Binary(Bytes),
}

impl WsMessage {
    /// The message's payload as bytes (the UTF-8 of a text message).
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            WsMessage::Text(text) => text.as_bytes(),
            WsMessage::Binary(bytes) => &bytes.0,
        }
    }
}

impl From<String> for WsMessage {
    fn from(text: String) -> WsMessage {
        WsMessage::Text(text)
    }
}

impl From<&str> for WsMessage {
    fn from(text: &str) -> WsMessage {
        WsMessage::Text(text.to_owned())
    }
}

impl From<Vec<u8>> for WsMessage {
    fn from(bytes: Vec<u8>) -> WsMessage {
        WsMessage::Binary(Bytes(bytes))
    }
}

/// Why a WebSocket could not be opened, or how it ended (ADR-047 §5).
///
/// | What happened | Variant |
/// |---|---|
/// | the upgrade was answered non-101, the URL is unusable, headers the platform cannot send | `Refused` |
/// | the connection dropped without a close frame (DNS, reset, TLS, timeout) | `Network` |
/// | the peer broke RFC 6455, or a reply did not decode | `Protocol` |
/// | the peer sent a close frame (1000 included), or the adapter closed past its backlog limit (1008) | `Closed` |
#[undra_macros::error]
#[undra(crate = "crate::root")]
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum WsError {
    /// The connection was not established. `status` is the HTTP status of the refused upgrade
    /// where the platform reports it (browsers do not).
    #[error("the WebSocket was refused: {message}")]
    Refused {
        /// The HTTP status of the refused upgrade, if the platform tells.
        status: Option<u16>,
        /// What the platform said.
        message: String,
    },
    /// The connection failed or dropped without a closing handshake; the text is the platform's.
    #[error("WebSocket network error: {0}")]
    Network(String),
    /// The peer broke the protocol (or a port reply did not decode); the text says how.
    #[error("WebSocket protocol error: {0}")]
    Protocol(String),
    /// The connection was closed with a close frame: `code` and `reason` are the frame's.
    #[error("the WebSocket was closed ({code}): {reason}")]
    Closed {
        /// The close code (RFC 6455 §7.4): 1000 normal, 1001 going away, 1008 policy, ...
        code: u16,
        /// The close reason, possibly empty.
        reason: String,
    },
}

/// The text of an unavailable port, with E0062's code and link (see `records::no_adapter`).
fn no_adapter(port: &str) -> String {
    format!(
        "the {port} port has no adapter registered (E0062: register one, see https://shreypdev.github.io/undra/docs/errors.html#E0062)"
    )
}

/// A port that cannot answer is an ordinary outcome (SPEC 6.3):
///
/// | `PortError` | `WsError` |
/// |---|---|
/// | `Unavailable` | `Network("the WebSocket port has no adapter registered (E0062: ..)")` |
/// | `Cancelled` | `Network("the WebSocket call was cancelled")` |
/// | `Decode(e)` | `Protocol("malformed port reply: <e>")` |
/// | `Failed(bytes)` | the decoded `WsError`, else `Protocol("the WebSocket port reported an error that does not decode")` |
impl From<PortError> for WsError {
    fn from(error: PortError) -> Self {
        if let PortError::Failed(bytes) = &error {
            return WsError::decode_exact(bytes).unwrap_or_else(|_| {
                WsError::Protocol(
                    "the WebSocket port reported an error that does not decode".to_owned(),
                )
            });
        }
        match error {
            PortError::Unavailable => WsError::Network(no_adapter("WebSocket")),
            PortError::Cancelled => WsError::Network("the WebSocket call was cancelled".to_owned()),
            PortError::Decode(why) => WsError::Protocol(format!("malformed port reply: {why}")),
            other => WsError::Network(format!("the WebSocket port call failed: {other}")),
        }
    }
}

/// Opens WebSocket connections on the platform (ADR-047). Use [`WsConnection`] rather than
/// calling it directly: it keeps the pull discipline and closes what it opened.
///
/// Every method has an error channel, so a platform without an adapter answers
/// `WsError::Network(..E0062..)` instead of panicking.
#[undra_macros::port(dispatcher_by_use)]
#[undra(crate = "crate::root")]
pub trait WebSocket {
    /// Opens a connection to `url` (`ws://` or `wss://`), offering `protocols` and sending
    /// `headers` with the upgrade request. Completes once the handshake succeeded.
    async fn connect(
        &self,
        url: String,
        protocols: Vec<String>,
        headers: Vec<Header>,
    ) -> Result<WsOpened, WsError>;
    /// Sends `message`; completes when the platform accepted it and its outbound buffer is
    /// under its high-water mark.
    async fn send(&self, conn: u32, message: WsMessage) -> Result<(), WsError>;
    /// The next messages, at least one and at most `max`, once there are any. `Ok([])` means
    /// the core closed the connection; an error ends the inbound stream.
    async fn receive(&self, conn: u32, max: u32) -> Result<Vec<WsMessage>, WsError>;
    /// Starts the closing handshake with `code` and `reason`; a pending `receive` then answers
    /// `Ok([])`. Closing a connection that already ended is not an error.
    async fn close(&self, conn: u32, code: u16, reason: String) -> Result<(), WsError>;
}

/// What [`WsConnection::connect`] offers the server besides the URL.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WsOptions {
    /// Subprotocols to offer (`Sec-WebSocket-Protocol`), in order of preference.
    pub protocols: Vec<String>,
    /// Headers for the upgrade request. Browsers cannot send any: the browser adapter refuses
    /// a connect that has headers (`Refused`) instead of dropping them.
    pub headers: Vec<Header>,
}

impl WsOptions {
    /// Offers `protocol`.
    #[must_use]
    pub fn with_protocol(mut self, protocol: impl Into<String>) -> WsOptions {
        self.protocols.push(protocol.into());
        self
    }

    /// Sends a header with the upgrade request.
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> WsOptions {
        self.headers.push(Header::new(name, value));
        self
    }
}

/// The close code of a connection dropped without [`WsConnection::close`] (1001, going away).
pub const GOING_AWAY: u16 = 1001;

struct Connection {
    port: Arc<dyn WebSocket>,
    conn: u32,
    protocol: String,
    weak: Option<WeakCtx>,
    closed: AtomicBool,
    taken: AtomicBool,
}

impl Drop for Connection {
    /// A connection nobody closed is closed with 1001 (going away), fire and forget, on the
    /// runtime that opened it (through a `WeakCtx`, so a dropped runtime is not pinned).
    fn drop(&mut self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(ctx) = self.weak.as_ref().and_then(|weak| weak.upgrade().ok()) else {
            return;
        };
        let port = self.port.clone();
        let conn = self.conn;
        ctx.spawn(async move {
            let _ = port.close(conn, GOING_AWAY, String::new()).await;
        });
    }
}

/// An open WebSocket connection (ADR-047).
///
/// Cheap to clone: clones share the connection. It closes (1001) when the last clone and its
/// [`messages`](WsConnection::messages) stream are dropped without [`close`](WsConnection::close).
#[derive(Clone)]
pub struct WsConnection {
    inner: Arc<Connection>,
}

impl fmt::Debug for WsConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WsConnection")
            .field("conn", &self.inner.conn)
            .field("protocol", &self.inner.protocol)
            .field("closed", &self.inner.closed.load(Ordering::Acquire))
            .finish()
    }
}

impl WsConnection {
    /// Opens a connection to `url` through the runtime's `WebSocket` port (a fake if one is
    /// bound, else the platform).
    pub async fn connect(
        ctx: &Ctx,
        url: &str,
        options: WsOptions,
    ) -> Result<WsConnection, WsError> {
        let port = crate::web_socket(ctx);
        let opened = port
            .connect(url.to_owned(), options.protocols, options.headers)
            .await?;
        Ok(WsConnection::from_parts(
            port,
            opened,
            Some(ctx.downgrade()),
        ))
    }

    /// Wraps a connection `port` already opened. `weak` is where a dropped connection is closed
    /// from (`None`: it is not closed on drop).
    pub fn from_parts(
        port: Arc<dyn WebSocket>,
        opened: WsOpened,
        weak: Option<WeakCtx>,
    ) -> WsConnection {
        WsConnection {
            inner: Arc::new(Connection {
                port,
                conn: opened.conn,
                protocol: opened.protocol,
                weak,
                closed: AtomicBool::new(false),
                taken: AtomicBool::new(false),
            }),
        }
    }

    /// The adapter's id of this connection.
    pub fn id(&self) -> u32 {
        self.inner.conn
    }

    /// The subprotocol the server selected, or `""`.
    pub fn protocol(&self) -> &str {
        &self.inner.protocol
    }

    /// Sends `message`; completes when the platform accepted it (ADR-047 §4).
    pub async fn send(&self, message: impl Into<WsMessage>) -> Result<(), WsError> {
        self.inner.port.send(self.inner.conn, message.into()).await
    }

    /// Sends a text message.
    pub async fn send_text(&self, text: impl Into<String>) -> Result<(), WsError> {
        self.send(WsMessage::Text(text.into())).await
    }

    /// Sends a binary message.
    pub async fn send_binary(&self, bytes: impl Into<Bytes>) -> Result<(), WsError> {
        self.send(WsMessage::Binary(bytes.into())).await
    }

    /// The inbound messages. The first call gets the stream; a later one gets a stream whose
    /// only item is `Err(Protocol(..))`, because one connection has one pull in flight.
    pub fn messages(&self) -> WsMessages {
        let taken = self.inner.taken.swap(true, Ordering::AcqRel);
        WsMessages {
            inner: self.inner.clone(),
            buffered: VecDeque::new(),
            pending: None,
            end: if taken {
                End::Failed(Some(WsError::Protocol(
                    "the inbound stream of this connection was already taken".to_owned(),
                )))
            } else {
                End::Open
            },
            pulls: 0,
        }
    }

    /// Starts the closing handshake with `code` and `reason`. The inbound stream then ends
    /// cleanly once it has delivered what arrived before.
    pub async fn close(&self, code: u16, reason: impl Into<String>) -> Result<(), WsError> {
        self.inner.closed.store(true, Ordering::Release);
        self.inner
            .port
            .close(self.inner.conn, code, reason.into())
            .await
    }
}

type Pull = Pin<Box<dyn Future<Output = Result<Vec<WsMessage>, WsError>> + Send>>;

enum End {
    Open,
    Clean,
    Failed(Option<WsError>),
}

/// The inbound side of a [`WsConnection`]: a [`Stream`] of `Result<WsMessage, WsError>`.
///
/// It ends with `None` after the core closed the connection, and with one `Err` when the peer
/// closed it (`Closed`), it dropped (`Network`) or the peer broke the protocol (`Protocol`),
/// after every message that arrived before. It holds the connection open while it lives.
pub struct WsMessages {
    inner: Arc<Connection>,
    buffered: VecDeque<WsMessage>,
    pending: Option<Pull>,
    end: End,
    pulls: u64,
}

impl fmt::Debug for WsMessages {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WsMessages")
            .field("conn", &self.inner.conn)
            .field("buffered", &self.buffered.len())
            .field("pulling", &self.pending.is_some())
            .field("pulls", &self.pulls)
            .finish()
    }
}

impl WsMessages {
    /// How many pulls (`receive` calls) this stream has issued.
    pub fn pulls(&self) -> u64 {
        self.pulls
    }

    /// How many messages it holds that the consumer has not taken yet.
    pub fn buffered(&self) -> usize {
        self.buffered.len()
    }

    /// Issues a pull when none is in flight, the stream is open and it holds fewer than
    /// [`LOW_WATER`] messages, and polls it once so the port call goes out now.
    fn pull(&mut self, cx: &mut Context<'_>) {
        while self.pending.is_none()
            && matches!(self.end, End::Open)
            && self.buffered.len() < LOW_WATER
        {
            let port = self.inner.port.clone();
            let conn = self.inner.conn;
            self.pulls += 1;
            let mut pull: Pull = Box::pin(async move { port.receive(conn, CREDIT).await });
            match pull.as_mut().poll(cx) {
                Poll::Pending => self.pending = Some(pull),
                Poll::Ready(outcome) => self.settle(outcome),
            }
        }
    }

    fn settle(&mut self, outcome: Result<Vec<WsMessage>, WsError>) {
        match outcome {
            Ok(messages) if messages.is_empty() => self.end = End::Clean,
            Ok(messages) => self.buffered.extend(messages),
            Err(error) => self.end = End::Failed(Some(error)),
        }
    }
}

impl Stream for WsMessages {
    type Item = Result<WsMessage, WsError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        if let Some(pending) = this.pending.as_mut() {
            if let Poll::Ready(outcome) = pending.as_mut().poll(cx) {
                this.pending = None;
                this.settle(outcome);
            }
        }
        if let Some(message) = this.buffered.pop_front() {
            this.pull(cx);
            return Poll::Ready(Some(Ok(message)));
        }
        this.pull(cx);
        if let Some(message) = this.buffered.pop_front() {
            this.pull(cx);
            return Poll::Ready(Some(Ok(message)));
        }
        match &mut this.end {
            End::Open => Poll::Pending,
            End::Clean => Poll::Ready(None),
            End::Failed(error) => Poll::Ready(error.take().map(Err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use undra_wire::Encode;

    #[test]
    fn errors_display_and_round_trip() {
        let all = [
            WsError::Refused {
                status: Some(403),
                message: "forbidden".into(),
            },
            WsError::Refused {
                status: None,
                message: "x".into(),
            },
            WsError::Network("reset".into()),
            WsError::Protocol("masked frame".into()),
            WsError::Closed {
                code: 1000,
                reason: "bye".into(),
            },
        ];
        for error in all {
            assert_eq!(WsError::decode_exact(&error.encode_to_vec()), Ok(error));
        }
        assert_eq!(
            WsError::Closed {
                code: 1001,
                reason: "away".into()
            }
            .to_string(),
            "the WebSocket was closed (1001): away"
        );
        assert_eq!(
            WsError::Network("dns".into()).to_string(),
            "WebSocket network error: dns"
        );
    }

    #[test]
    fn exact_bytes_of_the_records() {
        assert_eq!(
            WsMessage::Text("hi".into()).encode_to_vec(),
            [0, 0, 2, 0, 0, 0, b'h', b'i']
        );
        assert_eq!(
            WsMessage::Binary(Bytes(vec![7])).encode_to_vec(),
            [1, 0, 1, 0, 0, 0, 7]
        );
        assert_eq!(
            WsOpened {
                conn: 3,
                protocol: "p".into()
            }
            .encode_to_vec(),
            [3, 0, 0, 0, 1, 0, 0, 0, b'p']
        );
        assert_eq!(
            WsError::Closed {
                code: 1000,
                reason: String::new()
            }
            .encode_to_vec(),
            [3, 0, 0xe8, 0x03, 0, 0, 0, 0]
        );
        assert_eq!(
            WsError::Refused {
                status: Some(401),
                message: String::new()
            }
            .encode_to_vec(),
            [0, 0, 1, 0x91, 0x01, 0, 0, 0, 0]
        );
    }

    #[test]
    fn port_errors_map_onto_ws_errors() {
        assert!(matches!(
            WsError::from(PortError::Unavailable),
            WsError::Network(text) if text.contains("E0062") && text.contains("WebSocket")
        ));
        assert_eq!(
            WsError::from(PortError::Cancelled),
            WsError::Network("the WebSocket call was cancelled".into())
        );
        let closed = WsError::Closed {
            code: 4000,
            reason: "r".into(),
        };
        assert_eq!(
            WsError::from(PortError::Failed(closed.encode_to_vec())),
            closed
        );
        assert!(matches!(
            WsError::from(PortError::Failed(vec![9, 9])),
            WsError::Protocol(_)
        ));
    }

    #[test]
    fn message_conversions() {
        assert_eq!(WsMessage::from("a"), WsMessage::Text("a".into()));
        assert_eq!(
            WsMessage::from(vec![1u8]),
            WsMessage::Binary(Bytes(vec![1]))
        );
        assert_eq!(WsMessage::Text("ab".into()).as_bytes(), b"ab");
        let options = WsOptions::default()
            .with_protocol("chat")
            .with_header("Authorization", "Bearer t");
        assert_eq!(options.protocols, ["chat"]);
        assert_eq!(options.headers[0].name, "Authorization");
    }
}
