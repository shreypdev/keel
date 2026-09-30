#![allow(unused)]

use keel_macros as k;

#[k::api]
pub struct Todo {
    #[keel(defualt)]
    pub done: bool,
    #[keel(key = "id")]
    pub title: String,
}

#[k::api(colour = "red")]
pub struct Other {
    pub x: u8,
}

fn main() {}
