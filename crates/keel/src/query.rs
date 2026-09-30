//! The query layer: the traits `#[keel::query]` and `#[keel::mutation]` implement, and the
//! client behind `ctx.query()` and `ctx.mutate(..)` (SPEC 4.5, 5.3 and 9).
//!
//! This module is `keel-query` under its facade path. The traits used to be defined here; they
//! moved down into `keel-query`, which implements the client against them (the facade depends
//! on `keel-query`, not the other way round), and this re-export keeps every path stable:
//! `keel::query::QueryDef`, `keel::query::MutationDef`, `keel::query::BoxFuture` and
//! `keel::query::CacheValue` are the same items as before, and the macros' generated code keeps
//! naming them through `::keel::query`.
//!
//! See the `keel-query` crate documentation for the model (keys, entries, staleness, retries,
//! garbage collection, persistence) and for mutations and the offline queue.

pub use keel_query::{
    BACKOFF_BASE_MS, BACKOFF_MAX_MS, BoxFuture, CACHE_KEY_PREFIX, CacheValue, CacheView, CtxQuery,
    DEFAULT_GC_MS, INVALIDATE_METHOD_ID, Invalidate, JITTER_PERCENT, MutationBuilder, MutationDef,
    MutationRegistration, MutationVTable, PERSIST_DEBOUNCE_MS, QUEUE_KEY, QueryClient, QueryDef,
    QueryHandle, QueryRegistration, QueryStatus, QueryVTable, REFETCH_METHOD_ID, Settled,
    backoff_ms, cache_key, idempotency_key,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_cache_value<T: CacheValue>() {}

    #[test]
    fn wire_values_are_cache_values() {
        assert_cache_value::<u32>();
        assert_cache_value::<String>();
        assert_cache_value::<Vec<(String, u8)>>();
        assert_cache_value::<Option<keel_wire::Uuid>>();
    }

    #[test]
    fn the_facade_paths_are_the_keel_query_items() {
        // The same trait through both paths: an impl for one satisfies the other.
        fn same<T: keel_query::QueryDef>() -> u32 {
            <T as QueryDef>::ID
        }
        struct Q;
        impl QueryDef for Q {
            const ID: u32 = 9;
            const KEY: &'static str = "q";
            const STALE_MS: Option<u64> = None;
            const PERSIST: bool = false;
            const RETRY: u32 = 0;
            type Params = ();
            type Output = u8;
            type Error = u8;
            fn fetch(_: keel_runtime::Ctx, _: ()) -> BoxFuture<Result<u8, u8>> {
                Box::pin(async { Ok(1) })
            }
        }
        assert_eq!(same::<Q>(), 9);
    }
}
