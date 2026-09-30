//! M4: a record needs at least one field: zero-width items defeat length validation.
#![allow(unused)]

use keel_macros as k;

#[k::api]
pub struct Marker;

#[k::api]
pub struct Braces {}

#[k::api]
pub struct Parens();

fn main() {}
