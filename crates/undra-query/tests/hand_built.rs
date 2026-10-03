//! A query written by hand, without `#[undra::query]` (ADR-052). The macros submit the query
//! runtime's start-up hook and dispatch layer next to each definition, so a core whose only query
//! is a hand-written `QueryDef` links neither unless it submits them itself. What that core gets:
//!
//! * hydration on the first use of the query client (`ctx.query()`, `ctx.mutate(..)`, a platform
//!   constructing a handle) instead of at start-up, never no hydration at all;
//! * a platform reaches a registration submitted by hand (`QueryRegistration::of`) only through
//!   the layer, which it must submit next to it, as the macros and `bench/benches/query.rs` do.
//!
//! This binary declares no query with the macros and submits the layer but not the hook. It
//! describes the query in the schema by hand too (`Registration::Query`): a persisted entry carries
//! the closure of its query's type (ADR-037), so a query the schema does not describe is not
//! persisted at all.
#![forbid(unsafe_code)]

use undra_meta::{ParamMeta, QueryKind, QueryMeta, Registration, TypeRefMeta, ids::fnv1a64};
use undra_ports::fakes::{FakeClock, Fakes, StoreOp};
use undra_query::{
    BoxFuture, CACHE_KEY_PREFIX, CACHE_KEY_PREFIX_V1, CtxQuery, QueryDef, QueryRegistration,
};
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
static COUNT_META: QueryMeta = QueryMeta {
    name: "count",
    query_id: Count::ID,
    kind: QueryKind::Query,
    key: Count::KEY,
    params: &[ParamMeta {
        name: "n",
        ty: TypeRefMeta::U32,
    }],
    returns: TypeRefMeta::U32,
    stale_ms: Count::STALE_MS,
    persist: true,
    idempotent: false,
    interval_ms: None,
    poll_in_background: false,
    infinite: None,
};
inventory::submit! { Registration::Query(&COUNT_META) }
inventory::submit! { undra_query::__private::LAYER }

/// A runtime started like an app's, with `Count((7,))` persisted as 41 by an earlier run of this
/// very build (format 1, which hydration reads once and rewrites in format 2).
fn started_with_a_persisted_entry() -> (TestRuntime, Fakes) {
    let t = TestRuntime::new();
    let fakes = Fakes::new();
    fakes.install_test(&t);
    let mut stored = Writer::new();
    stored.write_u64(t.runtime().schema_hash());
    stored.write_i64(FakeClock::DEFAULT_NOW_MS);
    stored.write_bytes(&41_u32.encode_to_vec());
    let params = fnv1a64(&(7_u32,).encode_to_vec());
    fakes.kv.insert(
        format!("{CACHE_KEY_PREFIX_V1}{:08x}.{params:016x}", Count::ID),
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

/// Query handles across a restore (ADR-059): the reviver that builds them again is added with the
/// client, so for a core whose only queries are hand-written it exists from the client's first use,
/// not from start-up. A runtime that has none treats a recreation record as a store type it does
/// not have (left out and reported in `dropped`, the handle stays stale, the restore goes on); one
/// restored after the first use re-issues it. `QueryRegistration` documents the order.
#[test]
fn a_hand_written_query_handle_is_re_issued_only_once_the_client_has_been_used() {
    let construct = |t: &TestRuntime| {
        let reply = t.call_sync(
            CallTarget::Constructor {
                type_id: Count::ID,
                method_id: Count::ID,
            },
            1,
            &(7_u32,).encode_to_vec(),
        );
        assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
        Handle::decode_exact(&reply.body).expect("a handle")
    };
    let refetch = |t: &TestRuntime, handle: Handle| {
        t.call_sync(
            CallTarget::Method {
                handle,
                method_id: undra_query::REFETCH_METHOD_ID,
            },
            2,
            &[],
        )
        .status
    };

    let (old, _fakes) = started_with_a_persisted_entry();
    let handle = construct(&old);
    let snapshot = old.runtime().snapshot();
    assert_eq!(
        undra_wire::payload::Snapshot::decode(&mut undra_wire::Reader::new(&snapshot))
            .unwrap()
            .stores
            .iter()
            .filter(|s| s.recreation().is_some())
            .count(),
        1,
        "a hand-written query's handle is re-creatable too"
    );

    // Before the client's first use there is no reviver: the record is a store type this runtime
    // does not have.
    let (early, _fakes) = started_with_a_persisted_entry();
    let report = early.runtime().restore_with_report(&snapshot).unwrap();
    assert_eq!(
        (report.reissued, report.dropped.len()),
        (0, 1),
        "{report:?}"
    );
    assert_eq!(report.dropped[0].handles, [handle.0]);
    assert_eq!(refetch(&early, handle), ReplyStatus::BadRequest);

    // After it, the same snapshot is honoured.
    let (late, _fakes) = started_with_a_persisted_entry();
    let _client = late.ctx().query();
    let report = late.runtime().restore_with_report(&snapshot).unwrap();
    assert_eq!(
        (report.reissued, report.refused.len()),
        (1, 0),
        "{report:?}"
    );
    assert_eq!(refetch(&late, handle), ReplyStatus::Ok);
}
