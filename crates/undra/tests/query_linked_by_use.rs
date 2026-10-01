//! The query runtime is linked by the definitions that need it (ADR-052): a core that declares
//! no `#[undra::query]` and no `#[undra::mutation]` registers neither `undra-query`'s start-up
//! hook nor its dispatch layer, so the linker drops the query runtime and the core reads nothing
//! from `Kv` when it starts. (`crates/undra-query/tests/linked_by_use.rs` is the other half: a
//! core that declares them has both, and runs and consults each once; `hand_built.rs` next to it
//! is a core whose only query is written by hand.)
//!
//! What a core without queries leaves in `Kv`: an app that removes its last query or mutation
//! changes its schema hash, and before ADR-052 its next start-up deleted the cache entries and
//! the offline queue written under the old hash. Now nothing reads them: they stay in the store,
//! unused (no query can read an entry, no mutation can replay a queue item, of a schema that has
//! none), until a later version declares a query again and its start-up deletes them.
#![forbid(unsafe_code)]

use undra::meta::inventory;
use undra::prelude::*;
use undra::runtime::testing::TestRuntime;
use undra::runtime::{DispatchLayer, InitHook};
use undra_ports::fakes::Fakes;

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

#[test]
fn what_an_earlier_version_persisted_stays_in_the_store_unread() {
    // A cache entry and an offline queue written by a version that had queries (another
    // schema hash): a core with queries deletes both at start-up, this one never looks.
    let fakes = Fakes::new();
    let stale = [
        "undra.query.cache.00c0ffee.0000000000000001",
        "undra.query.queue",
    ];
    for key in stale {
        fakes.kv.insert(key, vec![0xde, 0xad]);
    }
    let t = TestRuntime::new();
    fakes.install_test(&t);
    t.run_init_hooks();
    t.run_pending();
    assert!(fakes.kv.ops().is_empty(), "{:?}", fakes.kv.ops());
    assert_eq!(fakes.kv.keys(), stale.map(str::to_owned).to_vec());
}
