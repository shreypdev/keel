//! L5: rejections say what is wrong in the terms the user wrote.
#![allow(unused)]

use std::pin::Pin;
use std::sync::Arc;

use undra_macros as k;

pub struct Counter;

// Receivers that are not `&self`: `&Arc<Self>` and `Pin<&Self>` were reported as a type error
// and as "self by value".
#[k::api]
impl Counter {
    pub fn new() -> Self {
        Counter
    }

    pub fn by_arc(self: &Arc<Self>) -> u32 {
        1
    }

    pub fn pinned(self: Pin<&Self>) -> u32 {
        2
    }

    // A constructor behind a type alias is not followed.
    pub fn alias() -> Res<Self> {
        Ok(Counter)
    }
}

type Res<T> = Result<T, std::io::Error>;

// A boxed stream asks for `impl Stream`, not "a concrete `#[undra::api]` type".
#[k::api]
pub fn boxed() -> Pin<Box<dyn undra::runtime::Stream<Item = u32> + Send>> {
    todo!()
}

// `Result` nested inside another type of a return is not "a return type".
#[k::api]
pub fn nested() -> Vec<Result<u32, String>> {
    Vec::new()
}

// `#[error]` belongs to `#[undra::error]`.
#[k::api]
pub enum Plain {
    #[error("not here")]
    A,
    B(#[from] u8),
}

// User types that share a name the mapper recognises are told to rename.
#[k::api]
pub fn instant(at: Instant) {}

#[k::api]
pub struct Instant {
    pub ms: u64,
}

fn main() {}
