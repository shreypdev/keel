//! M2: a store struct written before (or without) its `#[keel::api(store)]` impl block says what
//! is missing (E0011) instead of only "the trait bound `Todos: KeelObject` is not satisfied".
#![allow(unused)]

use keel::prelude::{Ctx, Signal};
use keel_macros as k;

#[k::store]
pub struct Todos {
    ctx: Ctx,
    count: Signal<u32>,
}

fn main() {}
