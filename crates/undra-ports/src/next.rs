//! [`next`]: the one stream combinator a core needs to read the inbound side of a port in a loop.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

use futures_core::Stream;

/// The next item of a stream, as a future: `while let Some(m) = next(&mut messages).await`.
///
/// `undra-ports` has no `StreamExt`; this is the one combinator a core needs to read a
/// `WsMessages` or an `SseEvents` in a loop.
pub fn next<S: Stream + Unpin>(stream: &mut S) -> Next<'_, S> {
    Next { stream }
}

/// The future of [`next`].
#[derive(Debug)]
pub struct Next<'a, S> {
    stream: &'a mut S,
}

impl<S: Stream + Unpin> Future for Next<'_, S> {
    type Output = Option<S::Item>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut *self.stream).poll_next(cx)
    }
}
