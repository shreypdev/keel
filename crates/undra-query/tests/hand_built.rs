//! A query written by hand, without `#[undra::query]` (ADR-052). The macros submit the query
//! runtime's start-up hook and dispatch layer next to each definition, so a core whose only query
//! is a hand-written `QueryDef` links neither unless it submits them itself. What that core gets:
//!
//! * hydration on the first use of the query client (`ctx.query()`, `ctx.mutate(..)`, a platform
//!   constructing a handle) instead of at start-up, never no hydration at all;
//! * a platform reaches a registration submitted by hand (`QueryRegistration::of`) only through
//!   the layer, which it must submit next to it, as the macros and `bench/benches/query.rs` do.
//!
//! This binary declares no query with the macros and submits the layer but not the hook.
#![forbid(unsafe_code)]

use undra_ports::fakes::{FakeClock, Fakes, StoreOp};
use undra_query::{BoxFuture, CACHE_KEY_PREFIX, CtxQuery, QueryDef, QueryRegistration, cache_key};
use undra_runtime::testing::TestRuntime;
use undra_runtime::{Ctx, InitHook, inventory};
use undra_wire::payload::{CallTarget, ReplyStatus};
use undra_wire::{Decode, Encode, Handle, Writer};

/// A persisted query whose fetch answers its parameter: a cached value is told apart from a
/// fetched one by being different.
struct Count;

impl QueryDef for Count {
    const ID: u32 = 0x00c0_ffee;
    const KEY: &'static str = "count:{n}";
    const STALE_MS: Option<u64> = Some(3_600_000);
    const PERSIST: bool = true;
    const RETRY: u32 = 0;
    type Params = (u32,);
    type Output = u32;
    type Error = String;

    fn fetch(_: Ctx, (n,): (u32,)) -> BoxFuture<Result<u32, String>> {
        Box::pin(async move { Ok(n) })
    }
}

// What the macros would have submitted, minus the start-up hook.
inventory::submit! { QueryRegistration::of::<Count>() }
inventory::submit! { undra_query::__private::LAYER }

/// A runtime started like an app's, with `Count((7,))` persisted as 41 by an earlier run.
fn started_with_a_persisted_entry() -> (TestRuntime, Fakes) {
    let t = TestRuntime::new();
    let fakes = Fakes::new();
    fakes.install_test(&t);
    let mut stored = Writer::new();
    stored.write_u64(t.runtime().schema_hash());
    stored.write_i64(FakeClock::DEFAULT_NOW_MS);
    stored.write_bytes(&41_u32.encode_to_vec());
    fakes.kv.insert(
        cache_key(Count::ID, &(7_u32,).encode_to_vec()),
        stored.into_vec(),
    );
    t.run_init_hooks();
    t.run_pending();
    (t, fakes)
}

fn listings(fakes: &Fakes) -> usize {
    fakes
        .kv
        .ops()
        .into_iter()
        .filter(|op| *op == StoreOp::List(CACHE_KEY_PREFIX.to_owned()))
        .count()
}

#[test]
fn the_start_up_hook_is_not_linked_here() {
    assert!(
        !inventory::iter::<InitHook>
            .into_iter()
            .any(|h| h.name == undra_query::__private::HYDRATE.name),
        "this binary must not submit the hook for the tests below to mean anything"
    );
}

#[test]
fn without_the_hook_the_first_use_of_the_client_hydrates() {
    let (t, fakes) = started_with_a_persisted_entry();
    assert_eq!(listings(&fakes), 0, "nothing ran at start-up");

    let client = t.ctx().query();
    t.run_pending();
    assert_eq!(listings(&fakes), 1, "{:?}", fakes.kv.ops());

    let handle = client.observe::<Count>((7,));
    assert_eq!(
        handle.data().get(),
        Some(41),
        "the persisted value, not a fetch"
    );
    t.run_pending();
    assert_eq!(handle.data().get(), Some(41), "fresh for an hour: no fetch");

    // Once per runtime, however many clients.
    let _again = t.ctx().query();
    t.run_pending();
    assert_eq!(listings(&fakes), 1);
}

#[test]
fn a_platform_constructs_the_handle_through_the_layer_submitted_with_it() {
    let (t, fakes) = started_with_a_persisted_entry();
    let reply = t.call_sync(
        CallTarget::Constructor {
            type_id: Count::ID,
            method_id: Count::ID,
        },
        1,
        &(7_u32,).encode_to_vec(),
    );
    assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    let handle = Handle::decode_exact(&reply.body).expect("a handle");
    assert_ne!(handle, Handle::NULL);
    // Constructing a handle is a first use too.
    t.run_pending();
    assert_eq!(listings(&fakes), 1, "{:?}", fakes.kv.ops());
}
