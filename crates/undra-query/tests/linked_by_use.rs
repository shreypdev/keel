//! The query runtime is linked by the definitions that need it (ADR-052). Each `#[undra::query]`
//! and `#[undra::mutation]` of the test core submits the hydration hook and the dispatch layer
//! next to its own registration; the runtime keeps one of each, so a core with fifteen
//! definitions still hydrates once and asks the layer once per call. (`crates/undra/tests/
//! query_linked_by_use.rs` is the other half: a core without queries registers neither.)

mod common;

use common::*;
use undra_ports::fakes::StoreOp;
use undra_query::CACHE_KEY_PREFIX;
use undra_runtime::{DispatchLayer, InitHook, inventory};

/// The definitions in `common`: 7 queries and 8 mutations.
const DEFINITIONS: usize = 15;

#[test]
fn every_definition_submits_the_hook_and_the_layer() {
    let hooks = inventory::iter::<InitHook>
        .into_iter()
        .filter(|h| h.name == "undra-query.hydrate")
        .count();
    assert_eq!(hooks, DEFINITIONS);
    let layers = inventory::iter::<DispatchLayer>
        .into_iter()
        .filter(|l| l.name == "undra-query")
        .count();
    assert_eq!(layers, DEFINITIONS);
}

#[test]
fn the_runtime_hydrates_once() {
    let h = Harness::new();
    h.settle();
    let listings = h
        .fakes
        .kv
        .ops()
        .into_iter()
        .filter(|op| *op == StoreOp::List(CACHE_KEY_PREFIX.to_owned()))
        .count();
    assert_eq!(listings, 1, "{:?}", h.fakes.kv.ops());
}
