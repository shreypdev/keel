//! The `Sse` port (ADR-047; feature `sse`): server-sent events (`text/event-stream`) as a
//! [`Stream`] whose pull is the core's credit.
//!
//! # Wire contract
//!
//! | Type | Wire form |
//! |---|---|
//! | [`SseEvent`] | `id Option<String>, event String, data String, retry_ms Option<u32>` |
//! | [`SseError`] | `u16` index: `Refused { status: Option<u16>, message: String }` 0, `Network(String)` 1, `Protocol(String)` 2, `Ended` 3 |
//!
//! The adapter parses the stream as the HTML standard says (fields `event`, `data`, `id`,
//! `retry`; comments and unknown fields ignored; `data` lines joined with `\n`; an event without
//! `event` is `"message"`; an empty `data` buffer dispatches nothing) and does **not** reconnect:
//! `Ended` tells the core the server finished the response, and the core reconnects with
//! [`subscribe`] and the last `id` it saw, after `retry_ms` if the server sent one.
//!
//! ```no_run
//! use undra_ports::sse::{self, SseError};
//! use undra_runtime::Ctx;
//!
//! async fn tail(ctx: &Ctx, last: Option<String>) -> Result<Option<String>, SseError> {
//!     let mut last_id = last;
//!     let mut events = sse::subscribe(ctx, "https://api.test/feed", Vec::new(), last_id.clone());
//!     while let Some(next) = undra_ports::next(&mut events).await {
//!         match next {
//!             Ok(event) => last_id = event.id.or(last_id),
//!             Err(SseError::Ended) => return Ok(last_id), // reconnect from here
//!             Err(other) => return Err(other),
//!         }
//!     }
//!     Ok(last_id)
//! }
//! ```

use core::fmt;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::VecDeque;
use std::sync::Arc;

use futures_core::Stream;
use undra_runtime::{Ctx, PortError, WeakCtx};
use undra_wire::Decode;

use crate::records::Header;

/// How many events one pull asks for (SPEC 3.7's initial grant).
pub const CREDIT: u32 = 16;

/// [`SseEvents`] issues the next pull while it holds fewer events than this.
pub const LOW_WATER: usize = 8;

/// One server-sent event.
#[undra_macros::api]
#[undra(crate = "crate::root")]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SseEvent {
    /// The event's `id` field, if the event (or an earlier one, per the standard's last-event-id
    /// buffer) set one. Send it back as `last_event_id` to resume.
    pub id: Option<String>,
    /// The event type: the `event` field, `"message"` when absent.
    pub event: String,
    /// The `data` lines, joined with `\n`.
    pub data: String,
    /// The `retry` field in milliseconds, when this event carried one: how long the server asks
    /// clients to wait before reconnecting.
    pub retry_ms: Option<u32>,
}

/// Why a server-sent event stream could not be opened, or how it ended (ADR-047 §5).
#[undra_macros::error]
#[undra(crate = "crate::root")]
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum SseError {
    /// The request was answered with a status other than 2xx (a 204 means "stop"), or the URL
    /// is unusable. `status` is absent when there was no HTTP answer.
    #[error("the event stream was refused: {message}")]
    Refused {
        /// The HTTP status, if there was an answer.
        status: Option<u16>,
        /// What the platform said.
        message: String,
    },
    /// The connection failed or dropped; the text is the platform's.
    #[error("event stream network error: {0}")]
    Network(String),
    /// The answer was not `text/event-stream`, was not UTF-8, or a port reply did not decode.
    #[error("event stream protocol error: {0}")]
    Protocol(String),
    /// The server ended the response. Reconnect with the last event id to resume.
    #[error("the server ended the event stream")]
    Ended,
}

/// A port that cannot answer is an ordinary outcome (SPEC 6.3):
///
/// | `PortError` | `SseError` |
/// |---|---|
/// | `Unavailable` | `Network("the Sse port has no adapter registered (E0062: ..)")` |
/// | `Cancelled` | `Network("the Sse call was cancelled")` |
/// | `Decode(e)` | `Protocol("malformed port reply: <e>")` |
/// | `Failed(bytes)` | the decoded `SseError`, else `Protocol(..)` |
impl From<PortError> for SseError {
    fn from(error: PortError) -> Self {
        if let PortError::Failed(bytes) = &error {
            return SseError::decode_exact(bytes).unwrap_or_else(|_| {
                SseError::Protocol("the Sse port reported an error that does not decode".to_owned())
            });
        }
        match error {
            PortError::Unavailable => SseError::Network(
                "the Sse port has no adapter registered (E0062: register one, see https://shreypdev.github.io/undra/docs/errors.html#E0062)"
                    .to_owned(),
            ),
            PortError::Cancelled => SseError::Network("the Sse call was cancelled".to_owned()),
            PortError::Decode(why) => SseError::Protocol(format!("malformed port reply: {why}")),
            other => SseError::Network(format!("the Sse port call failed: {other}")),
        }
    }
}

/// Opens server-sent event streams on the platform (ADR-047). Use [`subscribe`] rather than
/// calling it directly.
#[undra_macros::port(dispatcher_by_use)]
#[undra(crate = "crate::root")]
pub trait Sse {
    /// Requests `url` with `Accept: text/event-stream`, `headers` and, when given, the
    /// `Last-Event-ID` header. Completes with the stream's id once a 2xx `text/event-stream`
    /// answer arrived.
    async fn open(
        &self,
        url: String,
        headers: Vec<Header>,
        last_event_id: Option<String>,
    ) -> Result<u32, SseError>;
    /// The next events, at least one and at most `max`, once there are any. `Ok([])` means the
    /// core closed the stream; an error ends it.
    async fn next(&self, stream: u32, max: u32) -> Result<Vec<SseEvent>, SseError>;
    /// Stops the stream and releases the connection; a pending `next` answers `Ok([])`.
    /// Closing a stream that already ended is not an error.
    async fn close(&self, stream: u32) -> Result<(), SseError>;
}

type Open = Pin<Box<dyn Future<Output = Result<u32, SseError>> + Send>>;
type Pull = Pin<Box<dyn Future<Output = Result<Vec<SseEvent>, SseError>> + Send>>;

enum State {
    Opening(Open),
    Open(u32),
    Clean,
    Failed(Option<SseError>),
}

/// Subscribes to the event stream at `url`: a [`Stream`] of `Result<SseEvent, SseError>`.
///
/// The request goes out on the first poll. A failure to open is the stream's only item; after
/// that it ends with `Err(Ended)` when the server ends the response, another `Err` when the
/// connection fails, and `None` after [`SseEvents::close`]. Dropping it closes the stream (fire
/// and forget, through a `WeakCtx`).
pub fn subscribe(
    ctx: &Ctx,
    url: &str,
    headers: Vec<Header>,
    last_event_id: Option<String>,
) -> SseEvents {
    let port = crate::sse(ctx);
    let opener = port.clone();
    let url = url.to_owned();
    SseEvents {
        port,
        weak: Some(ctx.downgrade()),
        state: State::Opening(Box::pin(async move {
            opener.open(url, headers, last_event_id).await
        })),
        buffered: VecDeque::new(),
        pending: None,
        pulls: 0,
    }
}

/// The stream [`subscribe`] returns.
pub struct SseEvents {
    port: Arc<dyn Sse>,
    weak: Option<WeakCtx>,
    state: State,
    buffered: VecDeque<SseEvent>,
    pending: Option<Pull>,
    pulls: u64,
}

impl fmt::Debug for SseEvents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match &self.state {
            State::Opening(_) => "opening".to_owned(),
            State::Open(id) => format!("open({id})"),
            State::Clean => "closed".to_owned(),
            State::Failed(_) => "failed".to_owned(),
        };
        f.debug_struct("SseEvents")
            .field("state", &state)
            .field("buffered", &self.buffered.len())
            .field("pulls", &self.pulls)
            .finish()
    }
}

impl SseEvents {
    /// The adapter's id of the stream once it is open.
    pub fn id(&self) -> Option<u32> {
        match self.state {
            State::Open(id) => Some(id),
            _ => None,
        }
    }

    /// How many pulls (`next` calls) this stream has issued.
    pub fn pulls(&self) -> u64 {
        self.pulls
    }

    /// Closes the stream. Events that arrived before are still delivered; then it ends with
    /// `None`.
    pub async fn close(&mut self) -> Result<(), SseError> {
        match self.state {
            State::Open(id) => {
                self.state = State::Clean;
                self.pending = None;
                self.port.close(id).await
            }
            State::Opening(_) => {
                // The open may complete on the platform: dropping its future abandons the call
                // and the adapter's stream then waits for a pull that never comes until the
                // runtime shuts down. Finish the open, then close what it opened.
                let state = core::mem::replace(&mut self.state, State::Clean);
                if let State::Opening(open) = state {
                    if let Ok(id) = open.await {
                        return self.port.close(id).await;
                    }
                }
                Ok(())
            }
            State::Clean | State::Failed(_) => Ok(()),
        }
    }

    fn pull(&mut self, cx: &mut Context<'_>) {
        while let State::Open(id) = self.state {
            if self.pending.is_some() || self.buffered.len() >= LOW_WATER {
                return;
            }
            let port = self.port.clone();
            self.pulls += 1;
            let mut pull: Pull = Box::pin(async move { port.next(id, CREDIT).await });
            match pull.as_mut().poll(cx) {
                Poll::Pending => self.pending = Some(pull),
                Poll::Ready(outcome) => self.settle(outcome),
            }
        }
    }

    fn settle(&mut self, outcome: Result<Vec<SseEvent>, SseError>) {
        match outcome {
            Ok(events) if events.is_empty() => self.state = State::Clean,
            Ok(events) => self.buffered.extend(events),
            Err(error) => self.state = State::Failed(Some(error)),
        }
    }
}

impl Drop for SseEvents {
    fn drop(&mut self) {
        let State::Open(id) = self.state else { return };
        let Some(ctx) = self.weak.as_ref().and_then(|weak| weak.upgrade().ok()) else {
            return;
        };
        let port = self.port.clone();
        ctx.spawn(async move {
            let _ = port.close(id).await;
        });
    }
}

impl Stream for SseEvents {
    type Item = Result<SseEvent, SseError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        if let State::Opening(open) = &mut this.state {
            match open.as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(id)) => this.state = State::Open(id),
                Poll::Ready(Err(error)) => this.state = State::Failed(Some(error)),
            }
        }
        if let Some(pending) = this.pending.as_mut() {
            if let Poll::Ready(outcome) = pending.as_mut().poll(cx) {
                this.pending = None;
                this.settle(outcome);
            }
        }
        if this.buffered.is_empty() {
            this.pull(cx);
        }
        if let Some(event) = this.buffered.pop_front() {
            this.pull(cx);
            return Poll::Ready(Some(Ok(event)));
        }
        match &mut this.state {
            State::Opening(_) | State::Open(_) => Poll::Pending,
            State::Clean => Poll::Ready(None),
            State::Failed(error) => Poll::Ready(error.take().map(Err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use undra_wire::Encode;

    #[test]
    fn records_round_trip_with_exact_bytes() {
        let event = SseEvent {
            id: Some("7".into()),
            event: "message".into(),
            data: "a\nb".into(),
            retry_ms: Some(3000),
        };
        assert_eq!(
            SseEvent::decode_exact(&event.encode_to_vec()),
            Ok(event.clone())
        );
        let bare = SseEvent {
            id: None,
            event: String::new(),
            data: String::new(),
            retry_ms: None,
        };
        assert_eq!(bare.encode_to_vec(), [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(SseError::Ended.encode_to_vec(), [3, 0]);
        for error in [
            SseError::Refused {
                status: Some(204),
                message: "no content".into(),
            },
            SseError::Network("x".into()),
            SseError::Protocol("text/html".into()),
            SseError::Ended,
        ] {
            assert_eq!(SseError::decode_exact(&error.encode_to_vec()), Ok(error));
        }
        assert_eq!(
            SseError::Ended.to_string(),
            "the server ended the event stream"
        );
    }

    #[test]
    fn port_errors_map_onto_sse_errors() {
        assert!(matches!(
            SseError::from(PortError::Unavailable),
            SseError::Network(text) if text.contains("E0062") && text.contains("Sse")
        ));
        assert_eq!(
            SseError::from(PortError::Failed(SseError::Ended.encode_to_vec())),
            SseError::Ended
        );
        assert!(matches!(
            SseError::from(PortError::Failed(vec![7])),
            SseError::Protocol(_)
        ));
    }
}
