//! M3/E0063: `Option<Option<T>>` cannot be told apart from `Option<T>` on Kotlin and TypeScript.
#![allow(unused)]

use undra_macros as k;

#[k::api]
pub struct Reading {
    pub value: Option<Option<i32>>,
    pub many: Vec<Option<Box<Option<u8>>>>,
    pub fine: Option<Vec<Option<i32>>>,
}

fn main() {}
