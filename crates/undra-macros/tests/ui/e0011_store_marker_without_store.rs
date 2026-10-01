#![allow(unused)]

use undra::prelude::Ctx;
use undra_macros as k;

pub struct Plain {
    ctx: Ctx,
}

// `store` on the impl block of a struct that is not a `#[undra::store]`.
#[k::api(store)]
impl Plain {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }
}

fn main() {}
