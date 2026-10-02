//! ADR-058: `generic` marks a template, so it needs a type parameter to be instantiated with
//! (E0008), and a list belongs to a function: a type is instantiated under a name, by an alias.
#![allow(unused)]

use undra::prelude::{Ctx, Signal};
use undra_macros as k;

pub struct Cache;

#[k::api(generic)]
impl Cache {
    pub fn new() -> Self {
        Cache
    }
}

#[k::store(generic)]
pub struct Counter {
    ctx: Ctx,
    count: Signal<u32>,
}

pub struct Wrapper<T>(T);

#[k::api(generic(T = [u32]))]
impl<T> Wrapper<T> {
    pub fn new() -> Self {
        loop {}
    }
}

#[k::api(generic(T = [u32]))]
pub struct Holder<T> {
    pub value: T,
}

fn main() {}
