//! D1/NF2: a type that is not declared with `#[undra::api]` used in a record, a function and a
//! `Result`. The message that says so no longer calls it an alias, and an alias of a type that has
//! no Undra declaration (`type Id = u64`) is caught here, at the name, not later by `undra build`.
#![allow(unused)]

use undra_macros as k;

pub struct Plain {
    pub x: u32,
}

type Id = u64;

#[k::api]
pub struct Holder {
    pub plain: Plain,
}

#[k::api]
pub fn lookup(id: Id) -> u32 {
    0
}

fn main() {}
