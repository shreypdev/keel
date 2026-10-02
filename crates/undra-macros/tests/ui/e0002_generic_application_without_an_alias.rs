//! ADR-058 section 4: a generic type applied to a type parameter (`Page<T>`) is named through its
//! alias by the compiler, so a function that returns `Page<T>` works when `TodoPage` exists. For a
//! type no alias declares, the instantiation has no name (E0002), and the text says which alias to
//! write.
#![allow(unused)]

use undra_macros as k;

#[k::api]
#[derive(Clone)]
pub struct Todo {
    pub id: u32,
}

#[k::api]
#[derive(Clone)]
pub struct Note {
    pub id: u32,
}

#[k::api(generic)]
pub struct Page<T> {
    pub items: Vec<T>,
}

#[k::api]
pub type TodoPage = Page<Todo>;

/// `Page<Todo>` is named `TodoPage`; `Page<Note>` has no alias.
#[k::api(generic(T = [Todo, Note]))]
pub fn first_page<T: Clone>(items: Vec<T>) -> Page<T> {
    Page { items }
}

fn main() {}
