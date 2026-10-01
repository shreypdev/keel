//! The query runtime is linked by the definitions that need it (ADR-052): a core that declares
//! no `#[undra::query]` and no `#[undra::mutation]` registers neither `undra-query`'s start-up
//! hook nor its dispatch layer, so the linker drops the query runtime and the core reads nothing
//! from `Kv` when it starts. (`query_linked_by_use_with_queries.rs` is the other half: a core
//! that declares them has both, once each.)
#![forbid(unsafe_code)]

use undra::meta::inventory;
use undra::prelude::*;
use undra::runtime::testing::TestRuntime;
use undra::runtime::{DispatchLayer, InitHook};

/// A core with a store, a record and a function, as the `undra init` template has, and no query.
#[undra::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub text: String,
}

#[undra::store]
pub struct Notes {
    notes: Signal<Vec<Note>>,
}

#[undra::api(store)]
impl Notes {
    /// An empty list.
    pub fn new(_ctx: Ctx) -> Self {
        Self {
            notes: Signal::new(vec![]),
        }
    }

    /// Adds a note.
    pub fn add(&self, text: String) {
        self.notes.update(|list| list.push(Note { text }));
    }
}

/// A greeting.
#[undra::api]
pub fn hello(name: String) -> String {
    format!("hello {name}")
}

#[test]
fn a_core_without_queries_registers_no_query_hook_or_layer() {
    let hooks: Vec<&str> = inventory::iter::<InitHook>
        .into_iter()
        .map(|h| h.name)
        .collect();
    assert!(!hooks.contains(&"undra-query.hydrate"), "{hooks:?}");
    let layers: Vec<&str> = inventory::iter::<DispatchLayer>
        .into_iter()
        .map(|l| l.name)
        .collect();
    assert!(!layers.contains(&"undra-query"), "{layers:?}");
}

#[test]
fn and_it_starts_without_touching_a_port() {
    let t = TestRuntime::new();
    t.run_init_hooks();
    t.run_pending();
    assert!(
        t.host().port_calls().is_empty(),
        "start-up called a port: {:?}",
        t.host().port_calls()
    );
}
