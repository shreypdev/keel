#![allow(unused)]

use keel_macros as k;

#[k::port(event)]
pub trait Lifecycle {
    fn changed(&self, state: u8) -> bool;
    async fn resumed(&self);
}

fn main() {}
