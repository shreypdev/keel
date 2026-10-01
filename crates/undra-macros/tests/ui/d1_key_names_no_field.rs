//! D1/NF1: a `key` that names no field of the list's items. It used to be `rustc`'s E0609 with a
//! "similar name" suggestion that rewrote the string to a bare identifier (`key = id`, itself an
//! error). The macro cannot see the item type, so the lookup happens in a constant in the user's
//! crate and the diagnostic lists the fields there are, on the string where the key was written.
#![allow(unused)]

use undra::prelude::{Ctx, Signal};
use undra_macros as k;

#[k::api]
#[derive(Clone, PartialEq)]
pub struct Row {
    pub id: u32,
    pub title: String,
}

#[k::store]
pub struct Rows {
    ctx: Ctx,
    #[undra(key = "title")]
    fine: Signal<Vec<Row>>,
    #[undra(key = "id")]
    boxed: Signal<Vec<Box<Row>>>,
    #[undra(key = "idd")]
    typo: Signal<Vec<Row>>,
    #[undra(key = "n")]
    not_a_record: Signal<Vec<u32>>,
}

#[k::api(store)]
impl Rows {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            fine: Signal::new(Vec::new()),
            boxed: Signal::new(Vec::new()),
            typo: Signal::new(Vec::new()),
            not_a_record: Signal::new(Vec::new()),
        }
    }
}

fn main() {}
