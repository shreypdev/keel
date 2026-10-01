//! D1: `#[undra::api]` on a constructor or other associated function of a plain `impl` block. The
//! attribute goes on the impl block, which exposes every `pub fn` of it. A free function cannot name
//! `Self`, which is how the macro can tell.
#![allow(unused)]

use undra_macros as k;

pub struct Calc;

impl Calc {
    #[k::api]
    pub fn new() -> Self {
        Calc
    }
}

fn main() {}
