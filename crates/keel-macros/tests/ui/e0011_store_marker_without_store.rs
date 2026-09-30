#![allow(unused)]

use keel::prelude::Ctx;
use keel_macros as k;

pub struct Plain {
    ctx: Ctx,
}

// `store` on the impl block of a struct that is not a `#[keel::store]`.
#[k::api(store)]
impl Plain {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }
}

fn main() {}
