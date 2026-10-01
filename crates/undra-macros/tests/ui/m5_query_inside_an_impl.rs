//! M5: a query is a free function (the macro generates a struct next to it).
#![allow(unused)]

use undra::prelude::Ctx;
use undra_macros as k;

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

    #[undra::query(key = "todos")]
    pub async fn todos(ctx: &Ctx) -> Result<Vec<u32>, Failure> {
        Ok(vec![])
    }

    #[undra::mutation]
    pub async fn clear(ctx: &Ctx) -> Result<(), Failure> {
        Ok(())
    }
}

fn main() {}
