//! ADR-043: `interval` is at least one second. A poll refetches the whole result each time, so
//! anything faster is a stream; polling and paging belong to queries, not mutations.
#![allow(unused)]

use undra::prelude::Ctx;
use undra_macros as k;

#[k::query(key = "ticker", interval = "500ms")]
pub async fn ticker(ctx: &Ctx) -> Result<u32, TodoError> {
    Ok(1)
}

#[k::query(key = "now", interval = "0s")]
pub async fn now(ctx: &Ctx) -> Result<u32, TodoError> {
    Ok(1)
}

#[k::query(key = "later", interval = "soon")]
pub async fn later(ctx: &Ctx) -> Result<u32, TodoError> {
    Ok(1)
}

#[k::mutation(interval = "5s")]
pub async fn add(ctx: &Ctx) -> Result<u32, TodoError> {
    Ok(1)
}

fn main() {}
