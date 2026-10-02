//! Newtypes and named generic instantiations, written the way a core is (ADR-042): the template in
//! one module, its instantiations in another that imports only the template and the arguments.
#![forbid(unsafe_code)]

pub mod model {
    use std::collections::HashMap;

    use undra::prelude::Uuid;

    #[undra::api]
    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    pub struct UserId(pub Uuid);

    #[undra::api]
    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    pub struct Cursor(String);

    /// The template uses a type (`Cursor`) that the module of the alias does not import: the
    /// types of a template are checked where they resolve, not where it is instantiated.
    #[undra::api(generic)]
    #[derive(Clone, Debug, PartialEq)]
    pub struct Page<T> {
        pub items: Vec<T>,
        pub next: Option<Cursor>,
        pub by_owner: HashMap<UserId, Vec<T>>,
    }

    #[undra::api(generic)]
    #[derive(Clone, Debug, PartialEq)]
    pub enum Loadable<T> {
        Loading,
        Loaded(T),
        Failed(String),
    }

    #[undra::api]
    #[derive(Clone, Debug, PartialEq)]
    pub struct Todo {
        pub id: UserId,
        pub title: String,
    }
}

pub mod api {
    use std::collections::HashMap;

    use super::model::{Loadable, Page, Todo, UserId};

    #[undra::api]
    pub type TodoPage = Page<Todo>;

    #[undra::api]
    pub type IdPage = Page<UserId>;

    #[undra::api]
    pub type LoadableTodos = Loadable<Vec<Todo>>;

    #[undra::api]
    pub fn todos(count: u32) -> TodoPage {
        let _ = count;
        Page {
            items: Vec::new(),
            next: None,
            by_owner: Default::default(),
        }
    }

    #[undra::api]
    pub fn load() -> LoadableTodos {
        Loadable::Loading
    }

    #[undra::error]
    #[derive(Clone, Debug, PartialEq)]
    pub enum Failure {
        #[error("not found")]
        NotFound,
    }

    impl From<undra::runtime::PortError> for Failure {
        fn from(_: undra::runtime::PortError) -> Self {
            Failure::NotFound
        }
    }

    /// A newtype crosses as a query parameter and an instantiation as its result.
    #[undra::query(key = "todos:{owner}", stale = "30s")]
    pub async fn todos_of(ctx: &undra::prelude::Ctx, owner: UserId) -> Result<TodoPage, Failure> {
        let _ = (ctx, owner);
        Err(Failure::NotFound)
    }

    /// A port takes newtypes and instantiations by value.
    #[undra::port]
    pub trait Directory {
        async fn find(&self, id: UserId) -> Result<LoadableTodos, Failure>;
    }

    #[undra::api]
    pub fn owners(counts: HashMap<UserId, u32>) -> Option<UserId> {
        counts.into_keys().next()
    }
}

fn main() {}
