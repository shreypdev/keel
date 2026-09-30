//! The todo app of the blueprint, written the way a user writes it: records, an enum, an error,
//! a store with a computed signal, and commands. Compile-pass: every macro's output must
//! compile against the real runtime.
#![forbid(unsafe_code)]

use keel::prelude::*;

#[keel::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Todo {
    pub id: Uuid,
    pub title: String,
    pub done: bool,
}

#[keel::api]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Filter {
    All,
    Active,
    Done,
}

impl Filter {
    fn matches(self, todo: &Todo) -> bool {
        match self {
            Filter::All => true,
            Filter::Active => !todo.done,
            Filter::Done => todo.done,
        }
    }
}

#[keel::error]
#[derive(Clone, PartialEq)]
pub enum HttpError {
    #[error("network error: {0}")]
    Network(String),
    #[error("request timed out")]
    Timeout,
}

#[keel::error]
#[derive(Clone, PartialEq)]
pub enum TodoError {
    #[error("title cannot be empty")]
    EmptyTitle,
    #[error(transparent)]
    Http(#[from] HttpError),
}

#[keel::store(restore = "Self::assemble")]
pub struct Todos {
    ctx: Ctx,
    #[keel(key = "id")]
    todos: Signal<Vec<Todo>>,
    filter: Signal<Filter>,
    visible: Computed<Vec<Todo>>,
}

#[keel::api(store)]
impl Todos {
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(vec![]), Signal::new(Filter::All))
    }

    fn assemble(ctx: Ctx, todos: Signal<Vec<Todo>>, filter: Signal<Filter>) -> Self {
        let visible = Computed::new((&todos, &filter), |(todos, filter)| {
            todos.iter().filter(|t| filter.matches(t)).cloned().collect()
        });
        Self {
            ctx,
            todos,
            filter,
            visible,
        }
    }

    pub fn set_filter(&self, filter: Filter) {
        self.filter.set(filter);
    }

    pub fn add(&self, title: String) -> Result<Todo, TodoError> {
        let title = title.trim().to_owned();
        if title.is_empty() {
            return Err(TodoError::EmptyTitle);
        }
        let todo = Todo {
            id: Uuid([0; 16]),
            title,
            done: false,
        };
        self.todos.update(|list| list.push(todo.clone()));
        Ok(todo)
    }

    pub async fn refresh(&self, page: u32) -> Result<u32, HttpError> {
        let _ = &self.ctx;
        Ok(page)
    }
}

fn main() {}
