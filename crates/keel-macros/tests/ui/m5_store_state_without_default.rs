//! M5: a store field that is not a signal and not a `Ctx` is filled with `Default::default()`
//! when a snapshot is restored; without `Default` (and without a `restore` hook) the error is
//! branded and points at the field.
#![allow(unused)]

use keel::prelude::{Ctx, Signal};
use keel_macros as k;

pub struct Config {
    endpoint: String,
}

#[k::store]
pub struct Sync {
    ctx: Ctx,
    config: Config,
    count: Signal<u32>,
}

#[k::api(store)]
impl Sync {
    pub fn new(ctx: Ctx, endpoint: String) -> Self {
        Self {
            ctx,
            config: Config { endpoint },
            count: Signal::new(0),
        }
    }
}

fn main() {}
