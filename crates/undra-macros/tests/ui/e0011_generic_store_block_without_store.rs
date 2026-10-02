//! ADR-058: the impl block of a `#[undra::store(generic)]` must say it is a store. The check is a
//! constant the compiler evaluates at the alias (E0011).
#![allow(unused)]

use undra::prelude::*;
use undra_macros as k;

#[k::store(generic)]
pub struct Selection<T> {
    rows: Signal<Vec<T>>,
}

#[k::api(generic)]
impl<T: SignalValue> Selection<T> {
    pub fn new(ctx: Ctx) -> Self {
        Selection {
            rows: Signal::new(Vec::new()),
            __undra_cell: Default::default(),
        }
    }
}

#[k::api]
#[derive(Clone)]
pub struct Todo {
    pub id: u32,
}

#[k::api]
pub type TodoSelection = Selection<Todo>;

fn main() {}
