//! `Lazy<T>` (lazily paged lists) is not available in v1: the platform runtimes have no API for
//! it, so a store field of that type is rejected like `keel-bindgen` rejects it.
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

fn main() {}
