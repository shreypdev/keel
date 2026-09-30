#![allow(unused)]

use keel::prelude::Ctx;
use keel_macros as k;

#[k::query(key = "sync")]
pub fn sync_query(ctx: &Ctx) -> Result<u32, TodoError> {
    Ok(1)
}

#[k::query(key = "no-ctx")]
pub async fn no_ctx(page: u32) -> Result<u32, TodoError> {
    Ok(page)
}

#[k::query(key = "plain")]
pub async fn plain(ctx: &Ctx) -> u32 {
    1
}

fn main() {}
