//! ADR-042: `#[undra::api]` on a type alias instantiates a generic data type; any other alias is
//! E0007, and a template that was not declared `generic` is not found.
#![allow(unused)]

use undra_macros as k;

#[k::api]
pub type Id = u64;

#[k::api]
pub type Names = Vec<String>;

pub struct Plain<T>(pub T);

#[k::api]
pub type PlainInt = Plain<u32>;

fn main() {}
