//! Query client benchmarks: what it costs to observe a cached query, to publish a fetch result to
//! many observers, and to reach a query handle through the runtime's dispatch as a platform does.
//!
//! The query layer has no row of its own in the blueprint's budget table (section 14); these
//! numbers exist so a regression in the hot paths is visible, and so the crossing cost of a
//! handle (one constructor call, one change-set per transaction) can be compared with the
//! table's "handle method call" row.

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use undra::runtime::testing::TestRuntime;
use undra::wire::payload::CallTarget;
use undra_ports::fakes;
use undra_query::{BoxFuture, CtxQuery, QueryDef, REFETCH_METHOD_ID};
use undra_runtime::Ctx;
use undra_wire::{Decode, Encode, Handle};

/// A query that answers from memory: the benchmarks measure the client, not a network.
struct Count;

impl QueryDef for Count {
    const ID: u32 = 0xbe_c0_00_01;
    const KEY: &'static str = "count:{n}";
    const STALE_MS: Option<u64> = Some(60_000);
    const PERSIST: bool = false;
    const RETRY: u32 = 0;
    type Params = (u32,);
    type Output = Vec<u32>;
    type Error = String;

    fn fetch(_: Ctx, (n,): (u32,)) -> BoxFuture<Result<Vec<u32>, String>> {
        Box::pin(async move { Ok((0..n).collect()) })
    }
}

undra_runtime::inventory::submit! { undra_query::QueryRegistration::of::<Count>() }

fn runtime() -> TestRuntime {
    let t = TestRuntime::new();
    fakes::install(&t);
    t.run_init_hooks();
    t.run_pending();
    t
}

/// Observing a query whose data is cached and fresh, then letting go: an observer joining and
/// leaving, the common cost of a screen appearing.
fn observe_cached(c: &mut Criterion) {
    let t = runtime();
    let query = t.ctx().query();
    let keep = query.observe::<Count>((100,));
    t.run_pending();
    c.bench_function("query/observe_cached_and_release", |b| {
        b.iter(|| {
            let handle = query.observe::<Count>((100,));
            std::hint::black_box(handle.data().get());
        });
    });
    drop(keep);
}

/// A fetch result reaching 100 observers of one entry: one transaction, 100 handles updated.
fn publish_to_observers(c: &mut Criterion) {
    let t = runtime();
    let query = t.ctx().query();
    let handles: Vec<_> = (0..100).map(|_| query.observe::<Count>((100,))).collect();
    t.run_pending();
    c.bench_function("query/refetch_published_to_100_observers", |b| {
        b.iter(|| {
            handles[0].refetch();
            t.run_pending();
        });
    });
}

/// The platform's view: constructing a handle by its constructor call, and `refetch()` by its
/// pinned method id, through `Runtime::call_sync`.
fn platform_calls(c: &mut Criterion) {
    let t = runtime();
    let args = (10_u32,).encode_to_vec();
    let construct = || {
        let reply = t.call_sync(
            CallTarget::Constructor {
                type_id: Count::ID,
                method_id: Count::ID,
            },
            1,
            &args,
        );
        Handle::decode_exact(&reply.body).expect("the constructor answers a handle")
    };
    c.bench_function("query/platform_construct_and_release", |b| {
        b.iter_batched(
            || (),
            |()| {
                let handle = construct();
                t.runtime().release(handle.0);
                t.run_pending();
            },
            BatchSize::SmallInput,
        );
    });

    let handle = construct();
    t.run_pending();
    c.bench_function("query/platform_refetch_call", |b| {
        b.iter(|| {
            let reply = t.call_sync(
                CallTarget::Method {
                    handle,
                    method_id: REFETCH_METHOD_ID,
                },
                2,
                &[],
            );
            t.run_pending();
            std::hint::black_box(reply);
        });
    });
}

criterion_group!(
    benches,
    observe_cached,
    publish_to_observers,
    platform_calls
);
criterion_main!(benches);
