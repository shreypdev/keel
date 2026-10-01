//! [`FakeSse`]: a scripted server-sent events server for the `Sse` port (ADR-047 §8).

use core::fmt;
use core::future::poll_fn;
use core::task::{Poll, Waker};
use std::collections::{BTreeMap, VecDeque};

use parking_lot::Mutex;

use crate::Header;
use crate::sse::{Sse, SseError, SseEvent};

/// One `open` the core made (a snapshot).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeSseStream {
    /// The stream's id (from 1, in open order).
    pub stream: u32,
    /// The URL.
    pub url: String,
    /// The headers sent.
    pub headers: Vec<Header>,
    /// The `Last-Event-ID` the core resumed from.
    pub last_event_id: Option<String>,
    /// Whether the core closed it.
    pub closed_by_core: bool,
}

#[derive(Default)]
struct Stream {
    info: Option<FakeSseStream>,
    inbox: VecDeque<SseEvent>,
    end: Option<SseError>,
    pulls: Vec<u32>,
    delivered: u64,
    waker: Option<Waker>,
}

#[derive(Default)]
struct State {
    next_id: u32,
    refusals: Vec<(String, SseError)>,
    streams: BTreeMap<u32, Stream>,
}

/// A deterministic, in-memory event-stream server: every `open` of an `http(s)://` URL is
/// accepted unless [`refuse`](FakeSse::refuse) scripted otherwise; the test pushes events, ends
/// or fails streams, and reads the `Last-Event-ID` each open carried.
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use undra_ports::fakes;
/// use undra_ports::sse::{self, SseEvent};
/// use undra_runtime::testing::TestRuntime;
///
/// let t = TestRuntime::new();
/// let fakes = fakes::install(&t);
/// let seen = Arc::new(Mutex::new(Vec::new()));
/// let (ctx, sink) = (t.ctx(), seen.clone());
/// t.ctx().spawn(async move {
///     let mut events = sse::subscribe(&ctx, "https://feed.test/", Vec::new(), Some("41".into()));
///     while let Some(Ok(event)) = undra_ports::next(&mut events).await {
///         sink.lock().unwrap().push(event.data);
///     }
/// });
/// t.run_pending();
/// let stream = fakes.sse.last_stream().unwrap();
/// assert_eq!(fakes.sse.streams()[0].last_event_id.as_deref(), Some("41"));
/// fakes.sse.push(stream, SseEvent::message("hello"));
/// fakes.sse.end(stream);
/// t.run_pending();
/// assert_eq!(*seen.lock().unwrap(), ["hello"]);
/// ```
#[derive(Default)]
pub struct FakeSse {
    state: Mutex<State>,
}

impl fmt::Debug for FakeSse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeSse")
            .field("streams", &self.state.lock().streams.len())
            .finish()
    }
}

impl SseEvent {
    /// An event of type `"message"` with `data`, no id and no retry.
    pub fn message(data: impl Into<String>) -> SseEvent {
        SseEvent {
            id: None,
            event: "message".to_owned(),
            data: data.into(),
            retry_ms: None,
        }
    }

    /// This event with `id`.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> SseEvent {
        self.id = Some(id.into());
        self
    }

    /// This event with type `event`.
    #[must_use]
    pub fn with_event(mut self, event: impl Into<String>) -> SseEvent {
        self.event = event.into();
        self
    }
}

impl FakeSse {
    /// A server that accepts every stream.
    pub fn new() -> FakeSse {
        FakeSse::default()
    }

    /// Refuses opens of URLs that start with `url_prefix` with `error`.
    pub fn refuse(&self, url_prefix: impl Into<String>, error: SseError) -> &FakeSse {
        self.state.lock().refusals.push((url_prefix.into(), error));
        self
    }

    /// Every stream opened so far, in order.
    pub fn streams(&self) -> Vec<FakeSseStream> {
        self.state
            .lock()
            .streams
            .values()
            .filter_map(|s| s.info.clone())
            .collect()
    }

    /// The id of the newest stream.
    pub fn last_stream(&self) -> Option<u32> {
        self.state.lock().streams.keys().next_back().copied()
    }

    /// The server sends `event` on `stream`. Ignored once the stream ended.
    pub fn push(&self, stream: u32, event: SseEvent) {
        let waker = {
            let mut state = self.state.lock();
            let Some(s) = state.streams.get_mut(&stream) else {
                return;
            };
            if s.end.is_some() || s.info.as_ref().is_some_and(|i| i.closed_by_core) {
                return;
            }
            s.inbox.push_back(event);
            s.waker.take()
        };
        wake(waker);
    }

    /// The server ends the response: after the inbox, the stream ends with `Ended`.
    pub fn end(&self, stream: u32) {
        self.fail(stream, SseError::Ended);
    }

    /// The stream ends with `error` after its inbox.
    pub fn fail(&self, stream: u32, error: SseError) {
        let waker = {
            let mut state = self.state.lock();
            let Some(s) = state.streams.get_mut(&stream) else {
                return;
            };
            if s.end.is_none() {
                s.end = Some(error);
            }
            s.waker.take()
        };
        wake(waker);
    }

    /// The `max` of every pull on `stream`.
    pub fn pulls(&self, stream: u32) -> Vec<u32> {
        let state = self.state.lock();
        state
            .streams
            .get(&stream)
            .map(|s| s.pulls.clone())
            .unwrap_or_default()
    }

    /// How many events have been handed to the core on `stream`.
    pub fn delivered(&self, stream: u32) -> u64 {
        let state = self.state.lock();
        state.streams.get(&stream).map_or(0, |s| s.delivered)
    }
}

fn wake(waker: Option<Waker>) {
    if let Some(waker) = waker {
        waker.wake();
    }
}

fn unknown(stream: u32) -> SseError {
    SseError::Network(format!("no event stream {stream}"))
}

#[undra_macros::port]
impl Sse for FakeSse {
    async fn open(
        &self,
        url: String,
        headers: Vec<Header>,
        last_event_id: Option<String>,
    ) -> Result<u32, SseError> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(SseError::Refused {
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
        let stream = state.next_id;
        state.streams.insert(
            stream,
            Stream {
                info: Some(FakeSseStream {
                    stream,
                    url,
                    headers,
                    last_event_id,
                    closed_by_core: false,
                }),
                ..Stream::default()
            },
        );
        Ok(stream)
    }

    async fn next(&self, stream: u32, max: u32) -> Result<Vec<SseEvent>, SseError> {
        {
            let mut state = self.state.lock();
            let Some(s) = state.streams.get_mut(&stream) else {
                return Err(unknown(stream));
            };
            s.pulls.push(max);
        }
        poll_fn(|cx| {
            let mut state = self.state.lock();
            let Some(s) = state.streams.get_mut(&stream) else {
                return Poll::Ready(Err(unknown(stream)));
            };
            s.waker = None;
            if s.info.as_ref().is_some_and(|i| i.closed_by_core) {
                return Poll::Ready(Ok(Vec::new()));
            }
            if !s.inbox.is_empty() {
                let n = s.inbox.len().min(max.max(1) as usize);
                let batch: Vec<SseEvent> = s.inbox.drain(..n).collect();
                s.delivered += batch.len() as u64;
                return Poll::Ready(Ok(batch));
            }
            if let Some(end) = &s.end {
                return Poll::Ready(Err(end.clone()));
            }
            s.waker = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }

    async fn close(&self, stream: u32) -> Result<(), SseError> {
        let waker = {
            let mut state = self.state.lock();
            let Some(s) = state.streams.get_mut(&stream) else {
                return Err(unknown(stream));
            };
            if let Some(info) = s.info.as_mut() {
                info.closed_by_core = true;
            }
            s.inbox.clear();
            s.waker.take()
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
    fn opens_pulls_and_ends() {
        let sse = FakeSse::new();
        sse.refuse(
            "https://gone.test",
            SseError::Refused {
                status: Some(204),
                message: "stop".into(),
            },
        );
        let id = block_on(sse.open("https://feed.test".into(), vec![], Some("9".into()))).unwrap();
        assert!(matches!(
            block_on(sse.open("https://gone.test/x".into(), vec![], None)),
            Err(SseError::Refused {
                status: Some(204),
                ..
            })
        ));
        assert!(block_on(sse.open("ftp://x".into(), vec![], None)).is_err());
        sse.push(id, SseEvent::message("a").with_id("10"));
        sse.push(id, SseEvent::message("b").with_event("tick"));
        sse.end(id);
        let batch = block_on(sse.next(id, 16)).unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].id.as_deref(), Some("10"));
        assert_eq!(batch[1].event, "tick");
        assert_eq!(block_on(sse.next(id, 16)), Err(SseError::Ended));
        assert_eq!(sse.streams()[0].last_event_id.as_deref(), Some("9"));
        assert_eq!(sse.pulls(id), [16, 16]);
        assert_eq!(sse.delivered(id), 2);
    }

    #[test]
    fn close_ends_cleanly() {
        let sse = FakeSse::new();
        let id = block_on(sse.open("http://x".into(), vec![], None)).unwrap();
        block_on(sse.close(id)).unwrap();
        assert_eq!(block_on(sse.next(id, 1)), Ok(Vec::new()));
        assert!(sse.streams()[0].closed_by_core);
        assert_eq!(block_on(sse.close(7)), Err(unknown(7)));
    }
}
