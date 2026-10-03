//! ADR-058, the scope rule: the instantiation's dispatcher names the types of the template's
//! signatures where the alias stands, so in another module they must be in scope. `rustc` says what
//! is missing and offers the import; the guide says to declare the alias next to its template.
#![allow(unused)]

use undra_macros as k;

pub mod model {
    use undra_macros as k;

    #[k::api]
    pub struct Order {
        pub id: u32,
    }

    pub struct Cache<T>(Vec<T>);

    #[k::api(generic)]
    impl<T: Send + Sync + 'static> Cache<T> {
        pub fn new() -> Self {
            Cache(Vec::new())
        }

        pub fn put(&self, order: Order) {}
    }
}

#[k::api]
pub struct Todo {
    pub id: u32,
}

use model::Cache;

#[k::api]
pub type TodoCache = Cache<Todo>;

fn main() {}
