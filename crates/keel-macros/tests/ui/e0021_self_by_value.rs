#![allow(unused)]

use keel_macros as k;

pub struct Counter {
    value: u32,
}

#[k::api]
impl Counter {
    pub fn new() -> Self {
        Counter { value: 0 }
    }

    pub fn finish(self) -> u32 {
        self.value
    }
}

fn main() {}
