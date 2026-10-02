#![allow(unused)]

use undra_macros as k;

/// One field is a newtype (it is valid); two or more, and no field at all, are not.
#[k::api]
pub struct Meters(pub f64);

#[k::api]
pub struct Pair(pub f64, pub f64);

#[k::api]
pub struct Marker;

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
