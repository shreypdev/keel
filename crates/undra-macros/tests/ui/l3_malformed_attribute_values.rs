//! L3: helper attributes and macro arguments with a value of the wrong kind (or a value where
//! none belongs) get an Undra diagnostic, not `syn`'s "expected `,`" / "expected string literal".
#![allow(unused)]

use undra::prelude::{Ctx, Signal};
use undra_macros as k;

#[k::error]
#[derive(Clone)]
pub enum Failure {
    #[error("failed")]
    Failed,
}

#[k::api]
pub struct Row {
    pub id: u32,
    #[undra(default = true)]
    pub flag: bool,
}

#[k::store]
pub struct Rows {
    ctx: Ctx,
    #[undra(key = 5)]
    rows: Signal<Vec<Row>>,
    #[undra(no_coalesce = 1)]
    other: Signal<u32>,
}

#[k::api(crate)]
pub fn a() {}

#[k::api(crate = 5)]
pub fn b() {}

#[k::query(key = 7)]
pub async fn c(ctx: &Ctx) -> Result<u32, Failure> {
    Ok(1)
}

#[k::query(key = "k", retry = "3")]
pub async fn d(ctx: &Ctx) -> Result<u32, Failure> {
    Ok(1)
}

#[k::query(key = "k", stale = 30)]
pub async fn e(ctx: &Ctx) -> Result<u32, Failure> {
    Ok(1)
}

#[k::query(key = "k", persist = true)]
pub async fn f(ctx: &Ctx) -> Result<u32, Failure> {
    Ok(1)
}

#[k::port(sync = true)]
pub trait Clock {
    fn now(&self) -> u64;
}

#[k::store(restore = 5)]
pub struct Other {
    ctx: Ctx,
    n: Signal<u32>,
}

fn main() {}
