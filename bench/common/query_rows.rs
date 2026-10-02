//! The `query/*` rows the budgets test gates (ADR-043): what loading the next page of an infinite
//! query costs the core, against what the same rows cost as a recorded keyed patch on its own.
//!
//! * `query/infinite_append_page_50`: a platform's `fetch_next_page()` call on the handle of an
//!   infinite query whose list already holds 10,000 rows (a hand-written `InfiniteQueryDef`
//!   answering from memory, so the number is the client's, not a network's): the call, the fetch
//!   task, 50 rows appended with the recorded `push`, the change-set (a keyed patch of exactly those
//!   50 rows, asserted while the row is built) delivered to the host. Every run appends one page, so
//!   the list keeps growing: the cost must not.
//! * `query/keyed_push_50`: the same 50 rows appended to a keyed list of 10,000 by a store method
//!   (`call_sync`, 50 recorded `push`es, one change-set), what ADR-043 calls "a 50-op keyed patch".
//!   Its budget is ordinary; `[ratio."infinite_append_vs_keyed_push_50"]` of `budgets.toml` holds the
//!   first at no more than twice the second: O(page) with the machinery of a fetch on top, however
//!   long the list is.
#![allow(missing_docs, dead_code)]

use std::hint::black_box;

use undra::meta::ids;
use undra::prelude::*;
use undra::query::{
    BoxFuture, FETCH_NEXT_PAGE_METHOD_ID, InfiniteQueryDef, Page, PagedVTable, QueryDef,
    fetch_first_page, paged_vtable,
};
use undra::runtime::testing::TestRuntime;
use undra::signals::ALL_SIGNALS;
use undra::wire::payload::{CallTarget, ChangeOp, ReplyStatus};
use undra::wire::{Decode, Encode, Handle, KeyedPatch, Reader, Writer};
use undra_bench::workload::{Bench, Workload, plain};

use super::fixtures::{Item, title_of};

const ROWS: u64 = 10_000;
const PAGE: u64 = 50;

/// The rows of page `from`: ids `from..from + PAGE`.
fn rows(from: u64) -> Vec<Item> {
    (from..from + PAGE)
        .map(|id| Item {
            id,
            title: title_of(id as u32, 24),
            done: false,
        })
        .collect()
}

fn key_of(id: u64) -> u64 {
    ids::fnv1a64(&id.encode_to_vec())
}

/// An infinite query that answers from memory: a page of 50 rows at any cursor.
struct BenchFeed;

impl QueryDef for BenchFeed {
    const ID: u32 = 0xbe_c0_50_01;
    const KEY: &'static str = "bench_feed";
    const STALE_MS: Option<u64> = Some(3_600_000);
    const PERSIST: bool = false;
    const RETRY: u32 = 0;
    const PAGED: Option<&'static PagedVTable> = Some(paged_vtable::<Self>());
    type Params = ();
    type Output = Vec<Item>;
    type Error = String;

    fn fetch(ctx: Ctx, params: ()) -> BoxFuture<Result<Vec<Item>, String>> {
        fetch_first_page::<Self>(ctx, params)
    }
}

impl InfiniteQueryDef for BenchFeed {
    type Item = Item;
    type Cursor = u64;

    fn fetch_page(
        _: Ctx,
        _: (),
        cursor: Option<u64>,
    ) -> BoxFuture<Result<Page<Item, u64>, String>> {
        let from = cursor.unwrap_or(0);
        Box::pin(async move { Ok(Page::new(rows(from), Some(from + PAGE))) })
    }

    fn item_key(item: &Item) -> u64 {
        key_of(item.id)
    }
}

undra_runtime::inventory::submit! { undra::query::QueryRegistration::of::<BenchFeed>() }
undra_runtime::inventory::submit! { undra::query::__private::HYDRATE }
undra_runtime::inventory::submit! { undra::query::__private::LAYER }

/// A query that answers from memory, one handle of which a platform holds per parameter: what the
/// `snapshot/*_100_handles` rows carry (ADR-059).
struct HandleCount;

impl QueryDef for HandleCount {
    const ID: u32 = 0xbe_c0_50_02;
    const KEY: &'static str = "handle_count:{n}";
    const STALE_MS: Option<u64> = Some(3_600_000);
    const PERSIST: bool = false;
    const RETRY: u32 = 0;
    type Params = (u32,);
    type Output = u32;
    type Error = String;

    fn fetch(_: Ctx, (n,): (u32,)) -> BoxFuture<Result<u32, String>> {
        Box::pin(async move { Ok(n) })
    }
}

undra_runtime::inventory::submit! { undra::query::QueryRegistration::of::<HandleCount>() }

/// The query handles a snapshot keeps records of (ADR-059).
const HANDLES: u32 = 100;

/// A runtime holding [`HANDLES`] platform-constructed, observed handles of [`HandleCount`], and the
/// handles.
fn handles_rig() -> (TestRuntime, Vec<Handle>) {
    let t = rig();
    let mut handles = Vec::new();
    for n in 0..HANDLES {
        let reply = t.call_sync(
            CallTarget::Constructor {
                type_id: HandleCount::ID,
                method_id: HandleCount::ID,
            },
            n + 1,
            &(n,).encode_to_vec(),
        );
        assert_eq!(reply.status, ReplyStatus::Ok);
        let handle = Handle::decode_exact(&reply.body).expect("a handle");
        t.runtime().observe(handle.0, ALL_SIGNALS, true);
        handles.push(handle);
    }
    t.run_pending();
    t.take_change_sets();
    (t, handles)
}

/// The `snapshot/*_100_handles` rows (ADR-059): what keeping a recreation record of 100 query
/// handles costs a snapshot, and what re-issuing them costs a restore. They carry no store, so
/// they are the cost of the handles alone.
///
/// * `snapshot/encode_100_handles`: `Runtime::snapshot` with 100 live handles (a record each).
/// * `snapshot/restore_100_handles`: a restore into a runtime that does not hold them (a dev
///   reload, a web crash restart): 100 records checked and placed as dormant handles. The reset
///   releases the dormant handles, which a restore of the same snapshot would otherwise keep.
/// * `snapshot/restore_100_handles_live`: a restore into the runtime that holds them (a time
///   travel, an app's own `restore`): 100 live handles found and left alone.
pub fn snapshot_handles() -> Vec<Workload> {
    vec![
        Workload::new("snapshot/encode_100_handles", || {
            let (t, _handles) = handles_rig();
            let rt = t.runtime().clone();
            assert_eq!(
                undra::wire::payload::Snapshot::decode(&mut Reader::new(&rt.snapshot()))
                    .expect("a snapshot")
                    .stores
                    .len(),
                HANDLES as usize
            );
            plain(move || {
                black_box(rt.snapshot());
                let _ = &t;
            })
        }),
        Workload::new("snapshot/restore_100_handles", || {
            let (t, handles) = handles_rig();
            let rt = t.runtime().clone();
            let snapshot = rt.snapshot();
            for handle in &handles {
                rt.release(handle.0);
            }
            let report = rt.restore_with_report(&snapshot).expect("restores");
            assert_eq!(report.reissued, HANDLES as usize, "every handle re-issued");
            let rt2 = rt.clone();
            undra_bench::workload::with_reset(
                move || {
                    rt.restore(black_box(&snapshot)).expect("restore");
                    let _ = &t;
                },
                move || {
                    for handle in &handles {
                        rt2.release(handle.0);
                    }
                },
            )
        }),
        Workload::new("snapshot/restore_100_handles_live", || {
            let (t, _handles) = handles_rig();
            let rt = t.runtime().clone();
            let snapshot = rt.snapshot();
            let report = rt.restore_with_report(&snapshot).expect("restores");
            assert_eq!(report.reissued, HANDLES as usize, "every handle kept");
            plain(move || {
                rt.restore(black_box(&snapshot)).expect("restore");
                let _ = &t;
            })
        }),
    ]
}

/// A store with one keyed list and the method that appends a page to it.
#[undra::store]
pub struct PushFeed {
    #[undra(key = "id")]
    items: Signal<Vec<Item>>,
}

#[undra::api(store)]
impl PushFeed {
    pub fn new(_ctx: Ctx) -> Self {
        PushFeed {
            items: Signal::new(Vec::new()),
        }
    }

    /// Appends the 50 rows from `from`, one recorded `push` each.
    pub fn push_page(&self, from: u64) {
        // One transaction: one change-set of 50 ops (outside one, every `push` commits alone).
        undra::signals::txn(|| {
            for item in rows(from) {
                self.items.push(item);
            }
        });
    }

    /// Replaces the list with `count` rows (the setup).
    pub fn seed(&self, count: u64) {
        let all: Vec<Item> = (0..count)
            .map(|id| Item {
                id,
                title: title_of(id as u32, 24),
                done: false,
            })
            .collect();
        self.items.set(all);
    }
}

/// The workloads of the `query` group the budgets test gates.
pub fn rows_group() -> Vec<Workload> {
    vec![
        Workload::new("query/infinite_append_page_50", append_page_50),
        Workload::new("query/keyed_push_50", keyed_push_50),
    ]
}

fn rig() -> TestRuntime {
    let t = TestRuntime::new();
    undra::ports::fakes::install(&t);
    t.run_init_hooks();
    t.run_pending();
    t
}

fn decode_patch(bytes: &[u8]) -> KeyedPatch<Item> {
    let mut r = Reader::new(bytes);
    let patch = KeyedPatch::<Item>::decode(&mut r).expect("a keyed patch");
    r.finish().expect("a whole patch");
    patch
}

/// What the host was told by `change_sets`: the keyed patch of signal `data`, if any.
fn data_patch(change_sets: &[Vec<u8>], data: u32) -> Option<KeyedPatch<Item>> {
    let mut found = None;
    for set in change_sets {
        let decoded =
            undra::wire::payload::ChangeSet::decode(&mut Reader::new(set)).expect("a change-set");
        for entry in decoded.entries {
            if entry.signal_id == data && entry.op == ChangeOp::KeyedPatch {
                assert!(found.is_none(), "one patch per run");
                found = Some(decode_patch(&entry.value));
            }
        }
    }
    found
}

fn append_page_50() -> Box<dyn Bench> {
    let t = rig();
    let reply = t.call_sync(
        CallTarget::Constructor {
            type_id: BenchFeed::ID,
            method_id: BenchFeed::ID,
        },
        1,
        &().encode_to_vec(),
    );
    assert_eq!(reply.status, ReplyStatus::Ok);
    let handle = Handle::decode_exact(&reply.body).expect("a handle");
    t.run_pending();
    t.runtime().observe(handle.0, ALL_SIGNALS, true);
    let call = |id: u32| {
        let reply = t.call_sync(
            CallTarget::Method {
                handle,
                method_id: FETCH_NEXT_PAGE_METHOD_ID,
            },
            id,
            &[],
        );
        assert_eq!(reply.status, ReplyStatus::Ok);
        t.run_pending();
    };
    // 10,000 rows: the first page and 199 more.
    for id in 2..=ROWS / PAGE {
        call(id as u32);
    }
    t.take_change_sets();

    // One run ships a keyed patch of the 50 rows appended, and nothing else of the list.
    call(1_000_000);
    let sets = t.take_change_sets();
    let patch = data_patch(&sets, 0).expect("the page is a keyed patch");
    assert_eq!(patch.ops.len(), PAGE as usize, "50 rows, 50 ops");
    assert!(
        patch
            .ops
            .iter()
            .all(|op| matches!(op, undra::wire::PatchOp::Insert { .. })),
        "appended, not rewritten"
    );
    let shipped: usize = sets.iter().map(Vec::len).sum();
    assert!(
        shipped < 8_000,
        "a page is a patch of a few kilobytes, not the list: {shipped} bytes"
    );

    let mut id = 1_000_001_u32;
    plain(move || {
        id = id.wrapping_add(1);
        let reply = t.call_sync(
            CallTarget::Method {
                handle,
                method_id: FETCH_NEXT_PAGE_METHOD_ID,
            },
            black_box(id),
            &[],
        );
        black_box(reply);
        t.run_pending();
        black_box(t.take_change_sets());
    })
}

fn keyed_push_50() -> Box<dyn Bench> {
    let t = rig();
    let reply = t.call_sync(
        CallTarget::Constructor {
            type_id: ids::type_id("PushFeed"),
            method_id: ids::method_id("PushFeed", "new"),
        },
        1,
        &[],
    );
    assert_eq!(reply.status, ReplyStatus::Ok);
    let handle = Handle::decode_exact(&reply.body).expect("a handle");
    let call = |method: &str, id: u32, args: &[u8]| {
        let reply = t.call_sync(
            CallTarget::Method {
                handle,
                method_id: ids::method_id("PushFeed", method),
            },
            id,
            args,
        );
        assert_eq!(reply.status, ReplyStatus::Ok);
    };
    call("seed", 2, &ROWS.encode_to_vec());
    t.runtime().observe(handle.0, ALL_SIGNALS, true);
    t.take_change_sets();

    let mut from = ROWS;
    let mut args = Writer::new();
    from.encode(&mut args);
    call("push_page", 3, &args.into_vec());
    let sets = t.take_change_sets();
    let patch = data_patch(&sets, 0).expect("a keyed patch");
    assert_eq!(patch.ops.len(), PAGE as usize);

    from += PAGE;
    let mut id = 10_u32;
    plain(move || {
        id = id.wrapping_add(1);
        let reply = t.call_sync(
            CallTarget::Method {
                handle,
                method_id: ids::method_id("PushFeed", "push_page"),
            },
            black_box(id),
            &black_box(from).encode_to_vec(),
        );
        black_box(reply);
        from += PAGE;
        black_box(t.take_change_sets());
    })
}
