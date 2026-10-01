//! M2: a store struct written before (or without) its `#[undra::api(store)]` impl block says what
//! is missing (E0011) instead of only "the trait bound `Todos: UndraObject` is not satisfied".
#![allow(unused)]

use undra::prelude::{Ctx, Signal};
use undra_macros as k;

#[k::store]
pub struct Todos {
    ctx: Ctx,
    count: Signal<u32>,
}

fn main() {}
