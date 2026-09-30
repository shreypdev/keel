//! M5: `#[keel::api]` belongs on the impl block (or on a free function), never on a method.
#![allow(unused)]

use keel_macros as k;

pub struct Calc;

#[k::api]
impl Calc {
    pub fn new() -> Self {
        Calc
    }

    #[k::api]
    pub fn add(&self, a: i32, b: i32) -> i32 {
        a + b
    }
}

pub struct Other;

impl Other {
    #[k::api]
    pub fn twice(&self, a: i32) -> i32 {
        a * 2
    }
}

fn main() {}
