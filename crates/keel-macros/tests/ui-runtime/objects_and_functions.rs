//! Objects and free functions: constructors, sync and async methods, errors and streams.
#![forbid(unsafe_code)]

use std::collections::HashMap;

use keel::prelude::*;
use keel::runtime::Stream;

#[keel::error]
#[derive(Clone, PartialEq)]
pub enum CalcError {
    #[error("division by zero")]
    DivideByZero,
}

pub struct Calculator {
    base: i64,
}

#[keel::api]
impl Calculator {
    pub fn new(base: i64) -> Self {
        Calculator { base }
    }

    pub fn with_ctx(ctx: &Ctx, base: i64) -> Result<Self, CalcError> {
        let _ = ctx;
        Ok(Calculator { base })
    }

    pub fn add(&self, a: i64, b: i64) -> i64 {
        self.base + a + b
    }

    pub fn divide(&self, a: i64, b: i64) -> Result<i64, CalcError> {
        if b == 0 { Err(CalcError::DivideByZero) } else { Ok(a / b) }
    }

    pub fn summarize(&self, names: Vec<String>, flags: HashMap<String, bool>) -> String {
        format!("{} {}", names.len(), flags.len())
    }

    pub async fn slow_divide(&self, a: i64, b: i64) -> Result<i64, CalcError> {
        self.divide(a, b)
    }

    pub fn counts(&self, up_to: u32) -> impl Stream<Item = u32> {
        Counter { next: 0, up_to }
    }

    pub async fn later(&self, up_to: u32) -> Result<impl Stream<Item = u32>, CalcError> {
        Ok(Counter { next: 0, up_to })
    }
}

struct Counter {
    next: u32,
    up_to: u32,
}

impl Stream for Counter {
    type Item = u32;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<u32>> {
        if self.next < self.up_to {
            self.next += 1;
            std::task::Poll::Ready(Some(self.next))
        } else {
            std::task::Poll::Ready(None)
        }
    }
}

#[keel::api]
pub fn greet(name: String) -> String {
    format!("hello {name}")
}

#[keel::api]
pub async fn greet_later(ctx: &Ctx, name: String) -> Result<String, CalcError> {
    let _ = ctx;
    Ok(name)
}

fn main() {}
