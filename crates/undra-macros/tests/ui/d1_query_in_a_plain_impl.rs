//! D1: a query or mutation placed in a plain `impl` block (one that is not `#[undra::api]`) is the
//! one placement a macro cannot see. `rustc` used to answer with ten errors that never mention the
//! query; now it is three, and the last one names the rule and the fix. A signature that uses
//! `Self` gives the macro the answer directly.
#![allow(unused)]

use undra::prelude::Ctx;
use undra_macros as k;

#[k::error]
pub enum Failure {
    #[error("failed")]
    Failed,
}

pub struct Api;

impl Api {
    #[undra::query(key = "todos")]
    pub async fn todos(ctx: &Ctx) -> Result<Vec<u32>, Failure> {
        Ok(vec![])
    }
}

pub struct Admin;

impl Admin {
    #[undra::mutation]
    pub async fn clear(ctx: &Ctx) -> Result<(), Failure> {
        Ok(())
    }

    #[undra::query(key = "admins")]
    pub async fn admins(ctx: &Ctx) -> Result<Vec<Self>, Failure> {
        Ok(vec![])
    }
}

fn main() {}
