//! ADR-043: exactly one parameter is `#[undra(cursor)]`, of type `Option<C>`; it is not part of the
//! key; and only an `infinite` query has one.
#![allow(unused)]

use undra::prelude::Ctx;
use undra_macros as k;

#[k::query(key = "feed", infinite, item_key = "id")]
pub async fn no_cursor(ctx: &Ctx, tag: String) -> Result<undra::query::Page<Post>, FeedError> {
    Ok(undra::query::Page::last(vec![]))
}

#[k::query(key = "feed/{tag}", infinite, item_key = "id")]
pub async fn two_cursors(
    ctx: &Ctx,
    tag: String,
    #[undra(cursor)] after: Option<String>,
    #[undra(cursor)] before: Option<String>,
) -> Result<undra::query::Page<Post>, FeedError> {
    Ok(undra::query::Page::last(vec![]))
}

#[k::query(key = "feed", infinite, item_key = "id")]
pub async fn not_an_option(
    ctx: &Ctx,
    #[undra(cursor)] cursor: String,
) -> Result<undra::query::Page<Post>, FeedError> {
    Ok(undra::query::Page::last(vec![]))
}

#[k::query(key = "feed/{tag}/{cursor}", infinite, item_key = "id")]
pub async fn in_the_key(
    ctx: &Ctx,
    tag: String,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<undra::query::Page<Post>, FeedError> {
    Ok(undra::query::Page::last(vec![]))
}

#[k::query(key = "todos")]
pub async fn on_an_ordinary_query(
    ctx: &Ctx,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<u32, FeedError> {
    Ok(1)
}

fn main() {}
