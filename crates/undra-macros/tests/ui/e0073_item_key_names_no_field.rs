//! ADR-043: `item_key` names a field of the rows. The macro cannot see the fields of `Post`, so the
//! lookup happens in a constant in the user's crate, and the diagnostic lists the fields there
//! are, on the string where `item_key` was written (as for `#[undra(key = "..")]` on a list).
#![allow(unused)]

use undra::prelude::Ctx;
use undra_macros as k;

#[k::api]
#[derive(Clone, PartialEq)]
pub struct Post {
    pub id: u64,
    pub title: String,
}

/// A key that is a keyword names the raw field.
#[k::api]
#[derive(Clone, PartialEq)]
pub struct Tagged {
    pub r#type: String,
    pub id: u32,
}

#[k::error]
#[derive(Clone, PartialEq)]
pub enum FeedError {
    #[error("down")]
    Down,
}

#[k::query(key = "fine", infinite, item_key = "title")]
pub async fn fine(
    ctx: &Ctx,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<undra::query::Page<Post>, FeedError> {
    Ok(undra::query::Page::last(vec![]))
}

#[k::query(key = "keyword", infinite, item_key = "type")]
pub async fn keyword(
    ctx: &Ctx,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<undra::query::Page<Tagged>, FeedError> {
    Ok(undra::query::Page::last(vec![]))
}

#[k::query(key = "typo", infinite, item_key = "idd")]
pub async fn typo(
    ctx: &Ctx,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<undra::query::Page<Post>, FeedError> {
    Ok(undra::query::Page::last(vec![]))
}

#[k::query(key = "not a name", infinite, item_key = "not a name")]
pub async fn not_a_name(
    ctx: &Ctx,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<undra::query::Page<Tagged>, FeedError> {
    Ok(undra::query::Page::last(vec![]))
}

fn main() {}
