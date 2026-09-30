#![allow(unused)]

use keel::prelude::{Computed, Ctx, Signal};
use keel_macros as k;

#[k::store]
pub struct Todos {
    ctx: Ctx,
    all: Signal<Vec<u32>>,
    count: Computed<u32>,
}

fn main() {}
