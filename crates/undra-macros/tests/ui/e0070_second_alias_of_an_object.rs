//! ADR-058: one alias per instantiation of a generic object or store, as for data (E0070): a second
//! alias defines the constant that names the rule twice, and `rustc` reports it by that name.
#![allow(unused)]

use undra::prelude::*;
use undra_macros as k;

#[k::api]
#[derive(Clone)]
pub struct Todo {
    pub id: u32,
}

pub struct Cache<T>(Vec<T>);

#[k::api(generic)]
impl<T: Send + Sync + 'static> Cache<T> {
    pub fn new() -> Self {
        Cache(Vec::new())
    }
}

#[k::api]
pub type TodoCache = Cache<Todo>;

#[k::api]
pub type AnotherTodoCache = Cache<Todo>;

fn main() {}
