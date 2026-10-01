#![allow(unused)]

use undra_macros as k;

pub struct Counter {
    value: u32,
}

#[k::api]
impl Counter {
    pub fn new() -> Self {
        Counter { value: 0 }
    }

    pub fn increment(&mut self) {
        self.value += 1;
    }
}

fn main() {}
