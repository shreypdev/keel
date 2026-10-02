//! ADR-058: every rule of a hand-written object applies to a generic one, reported once at the
//! template: a method with `&mut self` and a function that is neither a method nor a constructor.
#![allow(unused)]

use undra_macros as k;

pub struct Cache<T>(Vec<T>);

#[k::api(generic)]
impl<T: Send + Sync + 'static> Cache<T> {
    pub fn new() -> Self {
        Cache(Vec::new())
    }

    pub fn clear(&mut self) {}

    pub fn helper() -> u32 {
        0
    }
}

fn main() {}
