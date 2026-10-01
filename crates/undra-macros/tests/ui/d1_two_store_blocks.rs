//! D1: a type takes one `#[undra::api]` impl block, a store included. The second block of a store
//! has no constructor of its own, and the message says that is the problem and where the methods go.
#![allow(unused)]

use undra::prelude::{Ctx, Signal};
use undra_macros as k;

#[k::store]
pub struct Counter {
    ctx: Ctx,
    count: Signal<i32>,
}

#[k::api(store)]
impl Counter {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            count: Signal::new(0),
        }
    }
}

#[k::api(store)]
impl Counter {
    pub fn bump(&self) {}
}

fn main() {}
