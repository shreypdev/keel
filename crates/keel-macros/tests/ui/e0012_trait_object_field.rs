#![allow(unused)]

use std::any::Any;

use keel_macros as k;

#[k::api]
pub struct Cart {
    pub items: Vec<Box<dyn Any>>,
}

fn main() {}
