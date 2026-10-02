#![allow(unused)]

use std::collections::HashMap;

use undra::prelude::Decimal;
use undra_macros as k;

#[k::api]
pub struct Histogram {
    pub buckets: HashMap<f64, u32>,
    /// A decimal compares by value and encodes by representation: not a key either.
    pub amounts: HashMap<Decimal, u32>,
}

fn main() {}
