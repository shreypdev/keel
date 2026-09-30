#![allow(unused)]

use std::any::Any;

use undra_macros as k;

#[k::api]
pub struct Cart {
    pub items: Vec<Box<dyn Any>>,
}

fn main() {}
