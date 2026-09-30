#![allow(unused)]

use std::collections::HashMap;

use keel_macros as k;

#[k::api]
pub struct Histogram {
    pub buckets: HashMap<f64, u32>,
}

fn main() {}
