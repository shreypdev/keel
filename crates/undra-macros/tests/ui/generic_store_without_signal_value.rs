//! ADR-058: a type parameter that stands in a signal needs the bound the signal needs,
//! `SignalValue`; without it `rustc` reports the unmet bound at the first use of the signal and
//! suggests restricting `T`.
#![allow(unused)]

use undra::prelude::*;
use undra_macros as k;

#[k::store(generic)]
pub struct Selection<T> {
    rows: Signal<Vec<T>>,
}

#[k::api(store, generic)]
impl<T> Selection<T> {
    pub fn new(ctx: Ctx) -> Self {
        Selection {
            rows: Signal::new(Vec::new()),
        }
    }
}

fn main() {}
