//! D1/NF1: a `key` that names no field of the list's items. It used to be `rustc`'s E0609 with a
//! "similar name" suggestion that rewrote the string to a bare identifier (`key = id`, itself an
//! error). The macro cannot see the item type, so the lookup happens in a constant in the user's
//! crate and the diagnostic lists the fields there are, on the string where the key was written.
//! A key that is a keyword names the raw field (`r#type`); one that is not an identifier at all is
//! the same single diagnostic.
#![allow(unused)]

use undra::prelude::{Ctx, Signal};
use undra_macros as k;

#[k::api]
#[derive(Clone, PartialEq)]
pub struct Row {
    pub id: u32,
    pub title: String,
}

/// A field named with a keyword: `key = "type"` reads `r#type`.
#[k::api]
#[derive(Clone, PartialEq)]
pub struct Tagged {
    pub r#type: String,
    pub id: u32,
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
    #[undra(key = "type")]
    keyword: Signal<Vec<Tagged>>,
    #[undra(key = "not a name")]
    not_a_name: Signal<Vec<Tagged>>,
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
            keyword: Signal::new(Vec::new()),
            not_a_name: Signal::new(Vec::new()),
        }
    }
}

fn main() {}
