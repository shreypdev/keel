//! ADR-042: a generic type is instantiated under a name, and a signature spells the name. Writing
//! the instantiation itself (`Page<Todo>`) is E0002, and the help is the alias to declare.
#![allow(unused)]

use std::collections::HashMap;

use undra_macros as k;

#[k::api]
pub struct Todo {
    pub title: String,
}

#[k::api(generic)]
pub struct Page<T> {
    pub items: Vec<T>,
}

#[k::api(generic)]
pub enum Loadable<T> {
    Loading,
    Loaded(T),
}

#[k::api]
pub struct Library {
    pub todos: Page<Todo>,
    pub nested: HashMap<String, Loadable<Vec<Todo>>>,
}

#[k::api]
pub fn first_page() -> Page<Todo> {
    Page { items: Vec::new() }
}

fn main() {}
