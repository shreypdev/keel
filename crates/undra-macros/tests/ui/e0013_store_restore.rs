//! M1: the store's own error is the only error, even with its `#[undra::api(store)]` impl block
//! next to it (the block uses members the failed `#[undra::store]` must still provide).
#![allow(unused)]

use undra::prelude::{Computed, Ctx, Signal};
use undra_macros as k;

#[k::store]
pub struct Todos {
    ctx: Ctx,
    all: Signal<Vec<u32>>,
    count: Computed<u32>,
}

#[k::api(store)]
impl Todos {
    pub fn new(ctx: Ctx) -> Self {
        let all = Signal::new(Vec::new());
        let count = Computed::new(&all, |all: &Vec<u32>| all.len() as u32);
        Self { ctx, all, count }
    }

    pub fn total(&self) -> u32 {
        self.count.get()
    }
}

fn main() {}
