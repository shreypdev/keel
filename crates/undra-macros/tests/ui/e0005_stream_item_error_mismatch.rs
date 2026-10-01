#![allow(unused)]

use std::pin::Pin;
use std::task::{Context, Poll};

use undra::runtime::Stream;
use undra_macros as k;

#[k::error]
#[derive(Debug)]
pub enum OpenError {
    #[error("closed")]
    Closed,
}

#[k::error]
#[derive(Debug)]
pub enum ItemError {
    #[error("expired")]
    Expired,
}

pub struct Nothing;

impl Stream for Nothing {
    type Item = Result<u32, ItemError>;
    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}

// ADR-036: a stream has one error type, whether it fails to open or part-way.
#[k::api]
pub fn ticks(n: u32) -> Result<impl Stream<Item = Result<u32, ItemError>>, OpenError> {
    Ok(Nothing)
}

fn main() {}
