//! `Lazy<T>` (lazily paged lists) is not available in v1: the platform runtimes have no API for
//! it, so a store field of that type is rejected like `keel-bindgen` rejects it. The impl block
//! that goes with the store adds no errors of its own (M1).
#![allow(unreachable_code)]
use keel::prelude::*;

#[keel::api]
pub struct Row {
    pub id: u32,
}

#[keel::store]
pub struct Feed {
    ctx: Ctx,
    older: Lazy<Row>,
}

#[keel::api(store)]
impl Feed {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            older: unimplemented!(),
        }
    }
}

fn main() {}
