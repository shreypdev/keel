//! M5/E0064: an object cannot be returned (or taken) as a value; it crosses as a handle.
#![allow(unused)]

use undra_macros as k;

pub struct Child;

#[k::api]
impl Child {
    pub fn new() -> Self {
        Child
    }

    pub fn name(&self) -> String {
        String::new()
    }
}

pub struct Parent;

#[k::api]
impl Parent {
    pub fn new() -> Self {
        Parent
    }

    pub fn child(&self) -> Child {
        Child
    }

    pub fn adopt(&self, child: Child) {}
}

fn main() {}
