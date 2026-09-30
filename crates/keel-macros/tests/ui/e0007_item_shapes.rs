#![allow(unused)]

use keel_macros as k;

#[k::api]
pub struct Meters(pub f64);

#[k::api]
pub enum Nothing {}

pub struct Counter;

#[k::api]
impl Counter {
    pub async fn new() -> Self {
        Counter
    }

    pub fn helper() -> u8 {
        1
    }
}

fn main() {}
