//! M3: a query caches the value it returns, so its success type cannot be `()` or an `Option`.
#![allow(unused)]

use undra::prelude::Ctx;
use undra_macros as k;

#[k::error]
#[derive(Clone)]
pub enum Failure {
    #[error("failed")]
    Failed,
}

#[k::api]
pub struct Todo {
    pub id: u32,
}

#[k::query(key = "ping")]
pub async fn ping(ctx: &Ctx) -> Result<(), Failure> {
    Ok(())
}

#[k::query(key = "find:{id}")]
pub async fn find(ctx: &Ctx, id: u32) -> Result<Option<Todo>, Failure> {
    Ok(None)
}

// A mutation may return `()`.
#[k::mutation]
pub async fn clear(ctx: &Ctx) -> Result<(), Failure> {
    Ok(())
}

fn main() {}
