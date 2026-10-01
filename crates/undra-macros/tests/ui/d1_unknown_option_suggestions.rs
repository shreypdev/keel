//! D1: an unknown option or argument names the closest one that exists; a misplaced option says
//! where it belongs; the attribute on the wrong kind of item says what it is and what to put it on
//! and points at the item's name; `store` on something that is not an impl block is reported on
//! the word `store`.
#![allow(unused)]

use undra::prelude::{Ctx, Signal};
use undra_macros as k;

#[k::api]
pub struct Todo {
    #[undra(defualt)]
    pub done: bool,
    #[undra(frobnicate)]
    pub title: String,
    #[undra(key = "id")]
    pub id: u32,
}

#[k::store(restor = "Self::rebuild")]
pub struct Store {
    ctx: Ctx,
    count: Signal<i32>,
}

#[k::store]
pub enum NotAStruct {
    A,
}

#[k::api]
pub type Alias = u32;

#[k::api(store)]
pub struct NotAnImpl {
    pub x: u8,
}

fn main() {}
