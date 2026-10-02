//! ADR-042: an alias of a template that was not declared with `#[undra::api]` registers nothing, so a
//! signature that spells it fails the identity check (E0061): the schema would name a type nobody
//! described.
#![allow(unused)]

use undra_macros as k;

#[k::api]
pub struct Todo {
    pub title: String,
}

#[k::api(generic)]
pub struct Page<T> {
    pub items: Vec<T>,
}

/// Registered: fine.
#[k::api]
pub type TodoPage = Page<Todo>;

/// Not registered: the same type as `TodoPage`, which is registered under that name.
pub type LostPage = Page<Todo>;

#[k::api]
pub fn pages() -> TodoPage {
    Page { items: Vec::new() }
}

#[k::api]
pub fn lost() -> LostPage {
    Page { items: Vec::new() }
}

fn main() {}
