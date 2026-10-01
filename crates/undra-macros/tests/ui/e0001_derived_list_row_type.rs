//! ADR-039: a `DerivedList` field needs the type of its rows.
#![allow(unused)]

use undra::prelude::{Ctx, DerivedList, Signal};
use undra_macros as k;

#[k::store(restore = "Self::assemble")]
pub struct Rows {
    rows: Signal<Vec<u32>>,
    #[undra(key = "id")]
    visible: DerivedList,
}

fn main() {}
