//! ADR-042: one alias per instantiation. A second alias of `Page<Todo>`, in any module, defines the
//! constant that names the rule twice, and `rustc` reports it by that name.
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

#[k::api]
pub type TodoPage = Page<Todo>;

mod elsewhere {
    use super::{Page, Todo};

    #[undra_macros::api]
    pub type AnotherTodoPage = Page<Todo>;
}

fn main() {}
