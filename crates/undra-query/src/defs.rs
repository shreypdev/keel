//! The contract `#[undra::query]` and `#[undra::mutation]` implement (SPEC 4.5 and 9).
//!
//! The macros generate a `<Name>Query` / `<Name>Mutation` struct implementing [`QueryDef`] /
//! [`MutationDef`] for each annotated function; [`QueryClient`](crate::QueryClient) is the
//! runtime side those definitions plug into (cache, staleness, retries, optimistic updates).

use core::future::Future;
use core::pin::Pin;

use undra_runtime::Ctx;
use undra_wire::{Decode, Encode};

use crate::paged::PagedVTable;

/// A boxed, sendable future.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// The bounds every value crossing the query cache must satisfy.
pub trait CacheValue: Encode + Decode + Clone + Send + Sync + 'static {}

impl<T: Encode + Decode + Clone + Send + Sync + 'static> CacheValue for T {}

/// A `#[undra::query]` function.
pub trait QueryDef: 'static {
    /// `fnv1a32("query.<fn name>")`.
    const ID: u32;
    /// The cache key template, `{param}` placeholders included.
    const KEY: &'static str;
    /// The staleness window in milliseconds.
    const STALE_MS: Option<u64>;
    /// Whether results are persisted.
    const PERSIST: bool;
    /// Retry attempts.
    const RETRY: u32;
    /// The polling interval in milliseconds (`interval = "30s"`, ADR-043), if the query polls by
    /// default: while an entry is observed, the app is `Active` (or the query polls in the
    /// background) and the client is online, the entry refetches this long after its previous
    /// fetch ended. Values below one second are raised to one second. `None` for a query that
    /// does not poll.
    const INTERVAL_MS: Option<u64> = None;
    /// Whether the query keeps polling while the app is in the background (`poll_in_background`,
    /// ADR-043).
    const POLL_IN_BACKGROUND: bool = false;
    /// The paging half of an [`InfiniteQueryDef`]: `Some` for `#[undra::query(infinite)]`, set by
    /// the macro through [`paged_vtable`](crate::paged_vtable). Not part of the stable API.
    #[doc(hidden)]
    const PAGED: Option<&'static PagedVTable> = None;
    /// The parameters (excluding `Ctx`) as a tuple.
    type Params: CacheValue;
    /// The success value.
    type Output: CacheValue;
    /// The error value.
    type Error: CacheValue;
    /// Runs the function.
    fn fetch(ctx: Ctx, params: Self::Params) -> BoxFuture<Result<Self::Output, Self::Error>>;
}

/// One page of an infinite query: its rows and the cursor of the page after it (ADR-043).
///
/// A plain Rust struct, never a schema type: `#[undra::query(infinite)]` recognises it by its
/// spelling in the success type of the function, and the platforms see only the rows (as the
/// handle's `data`) and `has_next_page`.
///
/// ```
/// use undra_query::Page;
///
/// let first: Page<u32> = Page::new(vec![1, 2, 3], Some("after-3".to_owned()));
/// assert_eq!(first.items, [1, 2, 3]);
/// assert_eq!(first.next.as_deref(), Some("after-3"));
/// let last: Page<u32, u64> = Page::last(vec![4]);
/// assert_eq!(last.next, None);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Page<T, C = String> {
    /// The rows of this page, in order.
    pub items: Vec<T>,
    /// The cursor to ask for the page after this one, or `None` when this was the last page.
    pub next: Option<C>,
}

impl<T, C> Page<T, C> {
    /// A page of `items` followed by the page at `next` (none: this is the last page).
    pub fn new(items: Vec<T>, next: Option<C>) -> Page<T, C> {
        Page { items, next }
    }

    /// The last page of a list: `items` and no next cursor.
    pub fn last(items: Vec<T>) -> Page<T, C> {
        Page { items, next: None }
    }
}

/// A `#[undra::query(infinite, ..)]` function (ADR-043): a query whose result is a list loaded
/// one page at a time, shown to the platforms as one keyed list that grows.
///
/// The macro implements this next to [`QueryDef`]; the `QueryDef` of an infinite query has
/// `Output = Vec<Item>` (the flattened list) and `fetch` returns the first page's rows. Observe
/// one with [`ctx.query().infinite::<Q>(params)`](crate::QueryClient::infinite).
pub trait InfiniteQueryDef: QueryDef<Output = Vec<<Self as InfiniteQueryDef>::Item>> {
    /// The row type `T` of `Page<T, C>`.
    type Item: CacheValue;
    /// The cursor type `C` of `Page<T, C>`.
    type Cursor: CacheValue;
    /// `refetch_pages = N`: a refetch loads at most this many pages (the pages beyond are
    /// dropped). `None`: every loaded page.
    const REFETCH_PAGES: Option<u32> = None;
    /// `persist_pages = N`: how many of the first pages a `persist` entry stores (at least 1).
    const PERSIST_PAGES: u32 = 1;
    /// Fetches the page at `cursor` (`None`: the first page).
    fn fetch_page(
        ctx: Ctx,
        params: Self::Params,
        cursor: Option<Self::Cursor>,
    ) -> BoxFuture<Result<Page<Self::Item, Self::Cursor>, Self::Error>>;
    /// The key of a row: the FNV-1a hash of its encoded `item_key` field, what the keyed list of
    /// the handle's `data` identifies rows by (SPEC 3.8).
    fn item_key(item: &Self::Item) -> u64;
}

/// The first page's rows of infinite query `Q`: what `QueryDef::fetch` of an infinite query
/// does. Generated code calls it.
#[doc(hidden)]
pub fn fetch_first_page<Q: InfiniteQueryDef>(
    ctx: Ctx,
    params: Q::Params,
) -> BoxFuture<Result<Q::Output, Q::Error>> {
    let fetch = Q::fetch_page(ctx, params, None);
    Box::pin(async move { fetch.await.map(|page| page.items) })
}

/// A `#[undra::mutation]` function.
pub trait MutationDef: 'static {
    /// `fnv1a32("mutation.<fn name>")`.
    const ID: u32;
    /// The invalidation key template.
    const KEY: &'static str;
    /// Retry attempts.
    const RETRY: u32;
    /// Whether the mutation is safe to replay.
    const IDEMPOTENT: bool;
    /// The parameters (excluding `Ctx`) as a tuple.
    type Input: CacheValue;
    /// The success value.
    type Output: CacheValue;
    /// The error value.
    type Error: CacheValue;
    /// Runs the function.
    fn execute(ctx: Ctx, input: Self::Input) -> BoxFuture<Result<Self::Output, Self::Error>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_cache_value<T: CacheValue>() {}

    #[test]
    fn wire_values_are_cache_values() {
        assert_cache_value::<u32>();
        assert_cache_value::<String>();
        assert_cache_value::<Vec<(String, u8)>>();
        assert_cache_value::<Option<undra_wire::Uuid>>();
    }
}
