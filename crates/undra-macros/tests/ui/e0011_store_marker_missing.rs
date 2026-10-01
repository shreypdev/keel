#![allow(unused)]

use undra::prelude::{Ctx, Signal};
use undra_macros as k;

#[k::store]
pub struct Todos {
    ctx: Ctx,
    count: Signal<u32>,
}

// The impl block of a store must be marked `store`.
#[k::api]
impl Todos {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            count: Signal::new(0),
            __undra_cell: Default::default(),
        }
    }
}

fn main() {}
