//! L5: a `key` that names no field of the list's items is reported on the string, where the
//! name was written.
#![allow(unused)]

use undra::prelude::{Ctx, Signal};
use undra_macros as k;

#[k::api]
#[derive(Clone, PartialEq)]
pub struct Row {
    pub id: u32,
}

#[k::store]
pub struct Rows {
    ctx: Ctx,
    #[undra(key = "identifier")]
    rows: Signal<Vec<Row>>,
}

#[k::api(store)]
impl Rows {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            rows: Signal::new(Vec::new()),
        }
    }
}

fn main() {}
