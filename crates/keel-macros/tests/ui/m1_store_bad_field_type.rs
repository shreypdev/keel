//! M1: a store with a field of an unsupported type reports that one error. Its impl block does
//! not add "no field named `__keel_cell`" or a false "no `#[keel::store]`" (E0011).
#![allow(unused)]

use keel::prelude::{Ctx, Signal};
use keel_macros as k;

#[k::store]
pub struct Cart {
    ctx: Ctx,
    note: Signal<&'static str>,
    count: Signal<u32>,
}

#[k::api(store)]
impl Cart {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            note: Signal::new(""),
            count: Signal::new(0),
        }
    }

    pub fn add(&self) {
        self.count.update(|n| *n += 1);
    }
}

fn main() {}
