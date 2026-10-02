//! ADR-042: only a struct or an enum can be generic, and only as a template (`generic`) that is
//! instantiated under a name. Everything else stays E0002, and the text points at that pattern.
#![allow(unused)]

use undra::prelude::*;
use undra_macros as k;

/// A data type without `generic`: the help says to mark it.
#[k::api]
pub struct Page<T> {
    pub items: Vec<T>,
}

/// An error enum is never generic.
#[k::error]
pub enum Failure<E> {
    #[error("failed")]
    Failed(E),
}

pub struct Cache;

/// An object, a method, a store, a function and a port.
#[k::api]
impl Cache {
    pub fn new() -> Self {
        Cache
    }

    pub fn get<T>(&self, value: T) -> u8 {
        0
    }
}

#[k::api]
pub fn first<T>(items: Vec<T>) -> u8 {
    0
}

#[k::store]
pub struct Counter<T> {
    ctx: Ctx,
    count: Signal<u32>,
    marker: Option<T>,
}

#[k::port]
pub trait Sink<T> {
    fn put(&self);
}

fn main() {}
