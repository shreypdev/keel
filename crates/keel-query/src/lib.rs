#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
//!
//! # Where things are
//!
//! | Item | What |
//! |---|---|
//! | [`QueryDef`], [`MutationDef`], [`CacheValue`], [`BoxFuture`] | the contract `#[keel::query]` and `#[keel::mutation]` implement |
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
//! ordinary change-set machinery (`keel-signals`), which is why a handle needs no special
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
//!   fetch, and read back when the runtime starts (see [`cache_key`]); entries written by
//!   another schema are dropped.
//! * **Mutations** are described in [`MutationBuilder`]; the offline queue in
//!   [`idempotency_key`].
//!
//! # Determinism
//!
//! The crate reads no clock and no random source of its own (Constitution R12): time comes from
//! the `Clock` port, jitter and idempotency keys from `Rng`, delays from `Ctx::sleep`. Under
//! `keel_ports::fakes` a test controls all of it.

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

use keel_runtime::{Ctx, InitHook, inventory};

/// Reads the persisted cache and queue when a runtime starts.
fn init(ctx: &Ctx) {
    let client = ctx.query();
    ctx.spawn(async move { client.hydrate().await });
}

inventory::submit! {
    InitHook { name: "keel-query.hydrate", run: init }
}
