//! ADR-043: the paging options come together. `infinite` needs `item_key` (a keyed list needs a
//! key), `item_key`, `refetch_pages` and `persist_pages` need `infinite`, and `persist_pages`
//! needs `persist`.
#![allow(unused)]

use undra::prelude::Ctx;
use undra_macros as k;

#[k::query(key = "feed", infinite)]
pub async fn no_item_key(
    ctx: &Ctx,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<undra::query::Page<Post>, FeedError> {
    Ok(undra::query::Page::last(vec![]))
}

#[k::query(key = "todos", item_key = "id", refetch_pages = 2)]
pub async fn not_infinite(ctx: &Ctx) -> Result<u32, FeedError> {
    Ok(1)
}

#[k::query(key = "saved", infinite, item_key = "id", persist_pages = 2)]
pub async fn no_persist(
    ctx: &Ctx,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<undra::query::Page<Post>, FeedError> {
    Ok(undra::query::Page::last(vec![]))
}

#[k::query(key = "zero", infinite, item_key = "id", refetch_pages = 0)]
pub async fn zero_pages(
    ctx: &Ctx,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<undra::query::Page<Post>, FeedError> {
    Ok(undra::query::Page::last(vec![]))
}

fn main() {}
