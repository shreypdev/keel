#![allow(unused)]

use undra_macros as k;

pub struct Button;

#[k::api]
impl Button {
    pub fn on_click(&self, callback: Box<dyn Fn(u32)>) {}

    pub fn describe(&self, printer: impl std::fmt::Display) {}
}

fn main() {}
