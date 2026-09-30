//! M5: one `#[keel::api] impl` block per type; the second block's duplicate definition names the
//! rule.
#![allow(unused)]

use keel_macros as k;

pub struct Calc;

#[k::api]
impl Calc {
    pub fn new() -> Self {
        Calc
    }

    pub fn add(&self, a: i32, b: i32) -> i32 {
        a + b
    }
}

#[k::api]
impl Calc {
    pub fn sub(&self, a: i32, b: i32) -> i32 {
        a - b
    }
}

fn main() {}
