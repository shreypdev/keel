//! ADR-043: the success type of an `infinite` query is `Page<T, C>` with `T` a record and `C` the
//! type of the cursor parameter's `Option<C>`.
#![allow(unused)]

use undra::prelude::Ctx;
use undra_macros as k;

#[k::query(key = "a", infinite, item_key = "id")]
pub async fn a_list(
    ctx: &Ctx,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<Vec<Post>, FeedError> {
    Ok(vec![])
}

#[k::query(key = "b", infinite, item_key = "id")]
pub async fn another_cursor_type(
    ctx: &Ctx,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<undra::query::Page<Post, u64>, FeedError> {
    Ok(undra::query::Page::last(vec![]))
}

#[k::query(key = "c", infinite, item_key = "id")]
pub async fn the_default_cursor_is_a_string(
    ctx: &Ctx,
    #[undra(cursor)] cursor: Option<u64>,
) -> Result<undra::query::Page<Post>, FeedError> {
    Ok(undra::query::Page::last(vec![]))
}

#[k::query(key = "d", infinite, item_key = "id")]
pub async fn rows_that_are_not_records(
    ctx: &Ctx,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<undra::query::Page<u32>, FeedError> {
    Ok(undra::query::Page::last(vec![]))
}

#[k::query(key = "e", infinite, item_key = "id")]
pub async fn no_page_at_all(
    ctx: &Ctx,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<u32, FeedError> {
    Ok(1)
}

fn main() {}
