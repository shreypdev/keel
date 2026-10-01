#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
//!
//! # Where things are
//!
//! | Item | What |
//! |---|---|
//! | [`QueryDef`], [`MutationDef`], [`CacheValue`], [`BoxFuture`] | the contract `#[undra::query]` and `#[undra::mutation]` implement |
//! | [`CtxQuery`], [`QueryClient`] | `ctx.query()` and `ctx.mutate(..)`: the cache and its operations |
//! | [`QueryHandle`], [`QueryStatus`] | the observable a query is watched through, and its five signals |
//! | [`MutationBuilder`], [`CacheView`], [`Invalidate`] | mutations, optimistic updates and invalidation targets |
//! | [`idempotency_key`] | the key of the idempotent mutation being run |
//! | [`QueryRegistration`], [`MutationRegistration`] | what the macros submit so platforms can find queries by id |
//! | [`REFETCH_METHOD_ID`], [`INVALIDATE_METHOD_ID`], [`cache_key`], [`QUEUE_KEY`] | wire and storage names |
//!
//! # How the pieces fit
//!
//! The cache maps a **key** (query id plus encoded parameters) to an **entry** holding the last
//! data and error as encoded bytes (and as the typed value, so observers do not decode it
//! again), when it was updated, and how many observers it has. A [`QueryHandle`] observes an
//! entry; its signals mirror the entry, and the platform mirrors the signals through the
//! ordinary change-set machinery (`undra-signals`), which is why a handle needs no special
//! support on the platform side beyond the generated class.
//!
//! * **Staleness.** An entry is stale when it has no data, when its last fetch failed, when it
//!   was invalidated, or when its `stale` window has passed (no window: always stale). Observing
//!   a stale entry fetches; observing a fresh one does not.
//! * **Dedup.** One fetch is in flight per key, however many observers there are.
//! * **Retries.** A failed fetch is retried `RETRY` times (default 3) after
//!   `min(1000 x 2^n, 30000)` ms, plus or minus 20% jitter from the `Rng` port, slept through
//!   the runtime's timers (see [`backoff_ms`]).
//! * **Triggers.** Besides observing: `Lifecycle::Active` refetches observed stale entries,
//!   `Connectivity` going online refetches all observed entries, `invalidate` refetches the
//!   observed entries it matches. The spec also lists `interval_ms`; neither `QueryDef` nor the
//!   schema carries an interval, so timed refetching is not in the v1 contract.
//! * **Garbage collection.** When the last observer of an entry goes, its fetch is cancelled and
//!   the entry is dropped after 5 minutes ([`QueryClient::set_gc_time`]) unless observed again.
//! * **Persistence.** `persist` entries are written to the `Kv` port 250 ms after a successful
//!   fetch, and read back when the runtime starts (see [`cache_key`]; the client waits a few
//!   seconds for a platform that registers its `Kv` adapter late); entries written by another
//!   schema are dropped. A garbage-collected entry leaves its persisted copy in the store.
//! * **Mutations** (`ctx.mutate(M, input)`, see [`MutationBuilder`]) run an optional
//!   *optimistic* update inside one transaction, so observers see the result at once; on failure
//!   the entries it wrote are restored exactly, in one more transaction, unless something else has
//!   written them since (a later optimistic mutation, a fetch result, a `set`; compared by a
//!   per-entry write stamp): those keep the newer write, so a rollback never removes the
//!   placeholder of a mutation that started after the failed one (see
//!   [`MutationBuilder::optimistic`] for the exact semantics); on success the mutation's own
//!   `key` (rendered with its input) and the `invalidates` targets are marked stale and the
//!   observed ones refetch. A mutation retries `RETRY` times (default 0) with the same
//!   backoff as a query.
//! * **The offline queue.** An `idempotent` mutation that fails with an `HttpError::Network` (its
//!   own error type usually wraps it; the client looks inside) **while the client believes the
//!   device is offline** is not failed: it is appended to a queue persisted under
//!   [`QUEUE_KEY`] and replayed first in first out when `Connectivity` reports online. The
//!   caller's `.await` keeps waiting and resolves with the result of the replay; its optimistic
//!   writes stay visible meanwhile and are rolled back if the replay is rejected. Everything else
//!   fails at once: a non-idempotent mutation is never queued, and neither is a network error
//!   while the platform says it is online (that is an ordinary failure). Every run of an
//!   idempotent mutation carries one key (a UUID v4 from the `Rng` port), visible inside its body
//!   as [`idempotency_key`], the same across retries and replays. A replay that fails for lack of
//!   network again stays queued: it waits for the next `online` event while the client is
//!   offline and retries with backoff while it is online. The `invalidates` targets of a queued
//!   mutation are remembered in memory only: after a restart the replay invalidates just the
//!   mutation's own key, and there is no optimistic update left to roll back.
//! * **What a platform sees.** For each query a `<Name>QueryHandle` object: its constructor
//!   (type id and method id are the query id) observes the entry, `refetch()` and `invalidate()`
//!   have the same method ids on every handle ([`REFETCH_METHOD_ID`], [`INVALIDATE_METHOD_ID`]),
//!   the five signals travel in ordinary change-sets, and releasing the object handle removes the
//!   observer. For each mutation an async function whose method id is the mutation id. None of
//!   this is in the runtime's static dispatch table (see `ADR-018` in `.10x/adrs`): the crate
//!   registers an `undra_runtime::DispatchLayer` and the macros submit a [`QueryRegistration`] /
//!   [`MutationRegistration`] per definition. Query handles are transient: a snapshot leaves them
//!   out and the platform re-creates them after a restore.
//!
//! # Deviations from SPEC 9 and 5.3
//!
//! * `ctx.query()` returns an owned [`QueryClient`] bound to the `Ctx`, not `&QueryClient`
//!   (the cache lives in the runtime's extension slot; see [`CtxQuery::query`]).
//! * `status` is derived: `Fetching` means a fetch is in flight *and there is nothing to show yet*.
//!   A refetch of an entry with data stays `Success` while `fetching` (signal 3) is `true`
//!   (see [`QueryStatus`]).
//! * The persisted queue starts with the schema hash, so arguments encoded by another schema are
//!   never replayed (SPEC 9 lists only `mutation_id`, params and the key per item).
//! * `interval_ms` is listed among the refetch triggers but neither `QueryDef` nor `QueryMeta`
//!   carries an interval, so timed refetching is not part of the v1 contract.
//! * The client assumes it is **online** until the platform says otherwise (platforms report the
//!   real state right after start-up), so a mutation made before the first connectivity event is
//!   attempted rather than parked.
//! * A failed fetch shows its typed error only after the retries run out; a fetch that
//!   panics has no typed error and shows `Error` with `error == None`.
//!
//! # Determinism
//!
//! The crate reads no clock and no random source of its own (Constitution R12): time comes from
//! the `Clock` port, jitter and idempotency keys from `Rng`, delays from `Ctx::sleep`. Under
//! `undra_ports::fakes` a test controls all of it.

mod client;
mod defs;
mod dispatch;
mod erased;
mod handle;
mod key;
mod mutation;
mod persist;
mod queue;
mod retry;
mod shared;
mod status;
mod walk;

pub use client::{CtxQuery, QueryClient};
pub use defs::{BoxFuture, CacheValue, MutationDef, QueryDef};
pub use dispatch::{INVALIDATE_METHOD_ID, REFETCH_METHOD_ID};
pub use erased::{MutationRegistration, MutationVTable, QueryRegistration, QueryVTable};
pub use handle::{QueryHandle, Settled};
pub use key::Invalidate;
pub use mutation::{CacheView, MutationBuilder};
pub use persist::{CACHE_KEY_PREFIX, QUEUE_KEY, cache_key};
pub use queue::idempotency_key;
pub use retry::{BACKOFF_BASE_MS, BACKOFF_MAX_MS, JITTER_PERCENT, backoff_ms};
pub use shared::{DEFAULT_GC_MS, PERSIST_DEBOUNCE_MS};
pub use status::QueryStatus;

use undra_runtime::{Ctx, InitHook, inventory};

/// Reads the persisted cache and queue when a runtime starts. The task holds the runtime weakly
/// (ADR-034), so an idle runtime whose owner lets go is freed even while hydration still waits
/// for a late `Kv` adapter.
pub(crate) fn init(ctx: &Ctx) {
    let shared = shared::shared_of(ctx.runtime());
    shared.start(ctx);
    let weak = ctx.downgrade();
    ctx.spawn(async move { shared.hydrate(&weak).await });
}

inventory::submit! {
    InitHook { name: "undra-query.hydrate", run: init }
}
