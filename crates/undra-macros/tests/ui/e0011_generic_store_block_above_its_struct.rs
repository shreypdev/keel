//! ADR-058: the impl block of a generic store hands its signatures to the macro its struct defines,
//! so the struct stands above it in the same module. A block that does not is reported by `rustc`
//! as a macro it cannot find, named after the rule (E0011).
#![allow(unused)]

use undra::prelude::*;
use undra_macros as k;

#[k::api(store, generic)]
impl<T: SignalValue> Selection<T> {
    pub fn new(ctx: Ctx) -> Self {
        Selection {
            rows: Signal::new(Vec::new()),
        }
    }
}

#[k::store(generic)]
pub struct Selection<T> {
    rows: Signal<Vec<T>>,
}

fn main() {}
