#![allow(unused)]

use keel::prelude::Ctx;
use keel_macros as k;

#[k::query(stale = "30s")]
pub async fn todos(ctx: &Ctx, page: u32) -> Result<Vec<u32>, TodoError> {
    Ok(vec![page])
}

#[k::mutation(stale = "30s")]
pub async fn add_todo(ctx: &Ctx, title: String) -> Result<u32, TodoError> {
    Ok(title.len() as u32)
}

#[k::query(key = "todos:{pgae}")]
pub async fn typo(ctx: &Ctx, page: u32) -> Result<u32, TodoError> {
    Ok(page)
}

fn main() {}
