//! `Lazy<T>` (lazily paged lists) is not available in v1: the platform runtimes have no API for
//! it, so a store field of that type is rejected like `undra-bindgen` rejects it. The impl block
//! that goes with the store adds no errors of its own (M1).
#![allow(unreachable_code)]
use undra::prelude::*;

#[undra::api]
pub struct Row {
    pub id: u32,
}

#[undra::store]
pub struct Feed {
    ctx: Ctx,
    older: Lazy<Row>,
}

#[undra::api(store)]
impl Feed {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            older: unimplemented!(),
        }
    }
}

fn main() {}
