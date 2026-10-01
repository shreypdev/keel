//! D1: an unknown option or argument names the closest one that exists; a misplaced option says
//! where it belongs; the attribute on the wrong kind of item says what it is and what to put it on.
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

fn main() {}
