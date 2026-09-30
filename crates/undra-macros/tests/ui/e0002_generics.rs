#![allow(unused)]

use undra_macros as k;

#[k::api]
pub struct Page<T> {
    pub items: Vec<T>,
}

#[k::api]
pub fn first<T>(items: Vec<T>) -> Option<T> {
    items.into_iter().next()
}

fn main() {}
