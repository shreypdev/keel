//! ADR-058: a store, an impl block, a query, a mutation and a callback with a type parameter say
//! what they may do instead (E0002).
#![allow(unused)]

use undra::prelude::*;
use undra_macros as k;

#[k::store]
pub struct Selection<T> {
    ctx: Ctx,
    rows: Signal<Vec<T>>,
}

pub struct Cache<T>(Vec<T>);

#[k::api]
impl<T> Cache<T> {
    pub fn new() -> Self {
        Cache(Vec::new())
    }
}

pub struct Mailbox;

#[k::api]
impl Mailbox {
    pub fn new() -> Self {
        Mailbox
    }
}

#[k::api]
impl Cache<u32> {
    pub fn count(&self) -> u32 {
        0
    }
}

#[k::query(key = "rows")]
pub async fn rows<T>(ctx: &Ctx) -> Result<Vec<u32>, undra::runtime::PortError> {
    Ok(Vec::new())
}

#[k::mutation]
pub async fn save<T>(ctx: &Ctx) -> Result<u32, undra::runtime::PortError> {
    Ok(0)
}

#[k::callback]
pub trait Listener<T> {
    fn changed(&self);
}

fn main() {}
