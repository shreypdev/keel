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
//!   this is in the runtime's static dispatch table (see `ADR-018` in `.10x/adrs`): the macros
//!   submit a [`QueryRegistration`] / [`MutationRegistration`] per definition, and with it this
//!   crate's `undra_runtime::DispatchLayer` and its start-up `undra_runtime::InitHook` (the runtime
//!   keeps one of each per name). Query handles are transient: a snapshot leaves them out and the
//!   platform re-creates them after a restore.
//! * **Linked by use.** Because the layer and the hook are submitted by `#[undra::query]` and
//!   `#[undra::mutation]`, not by this crate, a core that declares neither does not link the
//!   query runtime at all, and its start-up reads nothing from `Kv` (ADR-052: the layer was
//!   34 KB of the 136 KB gzipped hello-world web core). What such a core persisted while it had
//!   queries stays in the store unread (an app that removes its last query leaves its old cache
//!   entries and queue there; the next version that declares one deletes them, since their
//!   schema hash no longer matches). A [`QueryDef`] written by hand, in a core without
//!   macro-declared queries, is hydrated on the first use of the client (`ctx.query()`,
//!   `ctx.mutate(..)`) rather than at start-up. A [`QueryRegistration`] or
//!   [`MutationRegistration`] submitted by hand is reachable from a platform only through the
//!   layer: submit [`__private::LAYER`] (and [`__private::HYDRATE`], for start-up hydration)
//!   next to it, as the macros do; without the layer a platform's call is answered "unknown
//!   object type" or "unknown function".
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
mod inspect;
mod key;
mod mutation;
mod persist;
mod queue;
mod retry;
mod shared;
mod status;
mod storage;
mod walk;

pub use client::{CtxQuery, PersistStats, QueryClient};
pub use defs::{BoxFuture, CacheValue, MutationDef, QueryDef};
pub use dispatch::{INVALIDATE_METHOD_ID, REFETCH_METHOD_ID};
pub use erased::{MutationRegistration, MutationVTable, QueryRegistration, QueryVTable};
pub use handle::{QueryHandle, Settled};
pub use key::Invalidate;
pub use mutation::{CacheView, MutationBuilder};
pub use persist::{
    CACHE_KEY_PREFIX, CACHE_KEY_PREFIX_V1, DEAD_LETTER_KEY, QUEUE_KEY, QUEUE_KEY_V1,
    TYPES_KEY_PREFIX, cache_key, types_key,
};
pub use queue::{DeadLetter, RetryError, idempotency_key};
pub use retry::{BACKOFF_BASE_MS, BACKOFF_MAX_MS, JITTER_PERCENT, backoff_ms};
pub use shared::{DEFAULT_GC_MS, PERSIST_DEBOUNCE_MS};
pub use status::QueryStatus;
pub use storage::DEFAULT_MAX_PERSISTED_ENTRIES;

use undra_runtime::{Ctx, Runtime};

/// Reads the persisted cache and queue when a runtime starts. The task holds the runtime weakly
/// (ADR-034), so an idle runtime whose owner lets go is freed even while hydration still waits
/// for a late `Kv` adapter.
pub(crate) fn init(ctx: &Ctx) {
    let shared = shared::shared_of(ctx.runtime());
    shared.start(ctx);
    shared.spawn_hydration(ctx);
}

/// What `#[undra::query]` and `#[undra::mutation]` submit next to their registration, so the
/// query runtime is linked into a core only when the core declares a query or a mutation
/// (ADR-052). Not a stable API: generated code names it through `::undra::query::__private`.
///
/// Every definition submits both, so a core with several queries registers them several times;
/// the runtime runs an [`InitHook`](undra_runtime::InitHook) and consults a
/// [`DispatchLayer`](undra_runtime::DispatchLayer) once per name.
#[doc(hidden)]
pub mod __private {
    use undra_runtime::{DispatchLayer, InitHook};

    /// Hydrates the cache and the offline queue from the `Kv` port when a runtime starts.
    pub const HYDRATE: InitHook = InitHook {
        name: "undra-query.hydrate",
        run: crate::init,
    };

    /// Serves query handles (constructor, `refetch`, `invalidate`) and mutations by id.
    pub const LAYER: DispatchLayer = DispatchLayer {
        name: "undra-query",
        dispatch: crate::dispatch::dispatch,
    };
}

/// The `query` section of `stats_json`: cache and queue sizes, whether the stored queue was read,
/// and the persistence counters (`persist.write_failed`, `read_failed`, `dropped`, `migrated`,
/// `dead_lettered`; ADR-037, ADR-049). Registered on a runtime when its client is created
/// (`shared::shared_of`), so a core that never uses the query runtime does not link it (ADR-052).
pub(crate) fn stats_section(runtime: &Runtime) -> Option<String> {
    shared::existing(runtime).map(|shared| shared.stats_json())
}
