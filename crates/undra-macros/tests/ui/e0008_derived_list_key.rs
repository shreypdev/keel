//! ADR-039: a `DerivedList<T>` field must say which field identifies a row, and a keyed computed
//! list is pointed at `DerivedList<T>`.
#![allow(unused)]

use undra::prelude::{Computed, Ctx, DerivedList, Signal};
use undra_macros as k;

#[k::api]
#[derive(Clone, Debug)]
pub struct Todo {
    pub id: u32,
    pub done: bool,
}

#[k::store(restore = "Self::assemble")]
pub struct Todos {
    #[undra(key = "id")]
    todos: Signal<Vec<Todo>>,
    visible: DerivedList<Todo>,
    #[undra(key = "id")]
    done: Computed<Vec<Todo>>,
}

#[k::api(store)]
impl Todos {
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(Vec::new()))
    }

    fn assemble(_ctx: Ctx, todos: Signal<Vec<Todo>>) -> Self {
        let visible = todos.derive().filter(|t: &Todo| !t.done).build();
        let done = Computed::new(&todos, |l: &Vec<Todo>| l.iter().filter(|t| t.done).cloned().collect());
        Self { todos, visible, done }
    }
}

fn main() {}
