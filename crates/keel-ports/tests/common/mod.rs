//! Helpers shared by the integration tests.
#![allow(dead_code)]

use core::future::Future;
use core::task::{Context, Poll, Waker};

/// `"0000 01000000 75"` to bytes; whitespace is ignored.
pub fn hex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    assert_eq!(digits.len() % 2, 0, "odd number of hex digits in {text:?}");
    digits
        .chunks(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).unwrap();
            u8::from_str_radix(pair, 16).unwrap_or_else(|_| panic!("bad hex {pair:?}"))
        })
        .collect()
}

/// Polls a future that a fake answers immediately (no runtime needed).
pub fn ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("the future was not ready on its first poll"),
    }
}
