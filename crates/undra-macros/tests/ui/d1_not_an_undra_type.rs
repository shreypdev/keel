//! D1/NF2: a type that is not declared with `#[undra::api]` used in a record, a function and a
//! `Result`. The message that says so no longer calls it an alias, and an alias of a type that has
//! no Undra declaration (`type Id = u64`) is caught here, at the name, not later by `undra build`,
//! in a query's parameters too.
#![allow(unused)]

use undra::prelude::Ctx;
use undra_macros as k;

pub struct Plain {
    pub x: u32,
}

type Id = u64;

#[k::api]
pub struct Holder {
    pub plain: Plain,
}

#[k::api]
pub fn lookup(id: Id) -> u32 {
    0
}

#[k::error]
#[derive(Clone)]
pub enum Failure {
    #[error("failed")]
    Failed,
}

/// The checks of a query live in a named constant (it may sit in an `impl` block by mistake); they
/// still run.
#[k::query(key = "by-id:{id}")]
pub async fn by_id(ctx: &Ctx, id: Id) -> Result<u32, Failure> {
    Ok(0)
}

fn main() {}
