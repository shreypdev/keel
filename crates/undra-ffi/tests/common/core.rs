//! The core the boundary tests run against: real macro-generated code (an object with sync,
//! async, stream, panicking and failing methods, a store, custom ports and the three standard
//! ports the wasm shell binds natively). It is compiled into `tests/abi.rs` (the C ABI) and into
//! the `tests/fixture` cdylib (the wasm and JNI end-to-end runs), so every boundary sees the same
//! schema.
#![allow(dead_code)]

use undra::prelude::*;
use undra::runtime::Stream;

/// The standard clock port (SPEC 8), redeclared here because `undra-ports` is a separate crate;
/// ids derive from the trait name only, so the wasm built-in answers it.
#[undra::port(sync)]
pub trait Clock {
    fn now_ms(&self) -> i64;
    fn monotonic_ns(&self) -> u64;
}

/// The standard random-number port (SPEC 8).
#[undra::port(sync)]
pub trait Rng {
    fn fill(&self, len: u32) -> Bytes;
}

/// The standard log port (SPEC 8).
#[undra::port(sync)]
pub trait Log {
    fn log(&self, level: u8, target: String, message: String);
}

#[undra::error]
#[derive(Clone, Debug, PartialEq)]
pub enum CalcError {
    #[error("the calculation failed on purpose")]
    Failed,
}

/// Answered by the host, asynchronously.
#[undra::port]
pub trait Echo {
    async fn ping(&self, n: u32) -> u32;
}

/// Answered by the host, synchronously.
#[undra::port(sync)]
pub trait Sum {
    fn add(&self, a: u32, b: u32) -> u32;
}

pub struct Calculator {
    ctx: Ctx,
    base: i64,
}

#[undra::api]
impl Calculator {
    pub fn new(ctx: Ctx, base: i64) -> Self {
        Calculator { ctx, base }
    }

    pub fn add(&self, a: i64, b: i64) -> i64 {
        self.base.wrapping_add(a).wrapping_add(b)
    }

    pub async fn slow_add(&self, a: i64, b: i64) -> i64 {
        self.ctx.sleep(Duration::from_millis(20)).await;
        self.base.wrapping_add(a).wrapping_add(b)
    }

    /// An async method that is ready at once: measures the executor hop, not a timer.
    pub async fn ready_add(&self, a: i64, b: i64) -> i64 {
        self.base.wrapping_add(a).wrapping_add(b)
    }

    pub async fn never(&self) -> i64 {
        std::future::pending().await
    }

    pub fn ticks(&self, n: u32) -> impl Stream<Item = u32> {
        Ticks { next: 0, n }
    }

    pub fn boom(&self) -> i64 {
        panic!("kaboom")
    }

    pub async fn async_boom(&self) -> i64 {
        panic!("async kaboom")
    }

    pub fn fail(&self) -> Result<i64, CalcError> {
        Err(CalcError::Failed)
    }

    pub async fn ping_host(&self, n: u32) -> u32 {
        echo(&self.ctx).ping(n).await
    }

    pub fn sum_on_host(&self, a: u32, b: u32) -> u32 {
        sum(&self.ctx).add(a, b)
    }

    pub fn clock_now(&self) -> i64 {
        clock(&self.ctx).now_ms()
    }

    pub fn clock_monotonic(&self) -> u64 {
        clock(&self.ctx).monotonic_ns()
    }

    pub fn random_bytes(&self, len: u32) -> Bytes {
        rng(&self.ctx).fill(len)
    }

    pub fn log_line(&self, level: u8, message: String) {
        log(&self.ctx).log(level, "calculator".to_owned(), message);
    }

    /// Sleeps for a delay that does not fit in 32 bits (the wasm `timer_set` lo/hi split).
    pub async fn sleep_wide(&self, ms: u64) -> u64 {
        self.ctx.sleep(Duration::from_millis(ms)).await;
        ms
    }

    pub async fn sleep_ms(&self, ms: u32) -> u32 {
        self.ctx.sleep(Duration::from_millis(u64::from(ms))).await;
        ms
    }
}

struct Ticks {
    next: u32,
    n: u32,
}

impl Stream for Ticks {
    type Item = u32;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<u32>> {
        if self.next < self.n {
            self.next += 1;
            std::task::Poll::Ready(Some(self.next - 1))
        } else {
            std::task::Poll::Ready(None)
        }
    }
}

#[undra::store]
pub struct Counter {
    count: Signal<u32>,
}

#[undra::api(store)]
#[allow(clippy::new_without_default)]
impl Counter {
    pub fn new() -> Self {
        Counter {
            count: Signal::new(0),
        }
    }

    pub fn bump(&self) {
        self.count.update(|n| *n += 1);
    }
}

#[undra::api]
pub fn version() -> String {
    "undra-ffi test core 1".to_owned()
}
