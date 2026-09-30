#![allow(unused)]

use keel_macros as k;

#[k::api]
pub struct Label<'a> {
    pub text: &'a str,
}

#[k::api]
pub fn longest(a: &'static str) -> u8 {
    a.len() as u8
}

fn main() {}
