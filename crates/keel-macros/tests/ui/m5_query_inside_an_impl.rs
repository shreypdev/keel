//! M5: a query is a free function (the macro generates a struct next to it).
#![allow(unused)]

use keel::prelude::Ctx;
use keel_macros as k;

#[k::error]
pub enum Failure {
    #[error("failed")]
    Failed,
}

pub struct Api;

#[k::api]
impl Api {
    pub fn new() -> Self {
        Api
    }

    #[keel::query(key = "todos")]
    pub async fn todos(ctx: &Ctx) -> Result<Vec<u32>, Failure> {
        Ok(vec![])
    }

    #[keel::mutation]
    pub async fn clear(ctx: &Ctx) -> Result<(), Failure> {
        Ok(())
    }
}

fn main() {}
