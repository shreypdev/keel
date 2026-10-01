#![allow(unused)]

use undra_macros as k;

#[k::api]
pub struct Outcome {
    pub value: Result<u32, String>,
}

#[k::api]
pub fn apply(previous: Option<Result<u32, String>>) {}

fn main() {}
