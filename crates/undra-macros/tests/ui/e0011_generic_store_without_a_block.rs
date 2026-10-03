//! ADR-058: a `#[undra::store(generic)]` with no generic impl block exports no template; the alias
//! meets the struct, which is not a macro. `rustc` says so, and the errors page explains it under
//! E0011.
#![allow(unused)]

use undra::prelude::*;
use undra_macros as k;

#[k::store(generic)]
pub struct Selection<T> {
    rows: Signal<Vec<T>>,
}

#[k::api]
#[derive(Clone)]
pub struct Todo {
    pub id: u32,
}

#[k::api]
pub type TodoSelection = Selection<Todo>;

fn main() {}
