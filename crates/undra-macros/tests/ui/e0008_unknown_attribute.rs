#![allow(unused)]

use undra_macros as k;

#[k::api]
pub struct Todo {
    #[undra(defualt)]
    pub done: bool,
    #[undra(key = "id")]
    pub title: String,
}

#[k::api(colour = "red")]
pub struct Other {
    pub x: u8,
}

fn main() {}
