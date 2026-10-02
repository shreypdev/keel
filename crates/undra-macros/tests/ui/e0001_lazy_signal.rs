//! A `Lazy<T>` (ADR-043) is legal only as the type of a store field: it is a list the core owns and
//! the platforms page through, not a value. In a record field, a parameter, or inside a `Signal`,
//! `Computed` or `DerivedList` it is E0001, and the message says where it may stand. The impl block
//! that goes with the store adds no errors of its own (M1).
#![allow(unreachable_code)]
use undra::prelude::*;

#[undra::api]
pub struct Row {
    pub id: u32,
}

#[undra::api]
pub struct Shelf {
    pub rows: Lazy<Row>,
}

#[undra::api]
pub fn page_of(rows: Lazy<Row>) -> u32 {
    let _ = rows;
    0
}

#[undra::store]
pub struct Feed {
    ctx: Ctx,
    older: Computed<Lazy<Row>>,
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
