//! Helpers for driving generated futures and streams in tests.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::Arc;
use std::task::Wake;

use futures_core::Stream;

struct Noop;

impl Wake for Noop {
    fn wake(self: Arc<Self>) {}
}

/// Polls `future` to completion on the current thread. Panics if it stays pending (nothing in
/// these tests waits for another thread).
pub fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Noop));
    let mut cx = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    for _ in 0..10_000 {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
    }
    panic!("future did not complete");
}

/// Drains a stream to the end on the current thread.
pub fn collect<S: Stream + ?Sized>(stream: Pin<Box<S>>) -> Vec<S::Item> {
    let waker = Waker::from(Arc::new(Noop));
    let mut cx = Context::from_waker(&waker);
    let mut stream = stream;
    let mut items = Vec::new();
    for _ in 0..10_000 {
        match stream.as_mut().poll_next(&mut cx) {
            Poll::Ready(Some(item)) => items.push(item),
            Poll::Ready(None) => return items,
            Poll::Pending => {}
        }
    }
    panic!("stream did not finish");
}

/// A stream that yields the items of an iterator.
pub struct Iter<I>(pub I);

impl<I: Iterator + Unpin> Stream for Iter<I> {
    type Item = I::Item;
    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.0.next())
    }
}

/// A stream over `items`.
pub fn stream_of<T>(items: Vec<T>) -> Iter<std::vec::IntoIter<T>> {
    Iter(items.into_iter())
}
