#![allow(unused)]

use undra_macros as k;

#[k::port]
pub trait Http {
    async fn request(&self, url: &str, on_progress: Box<dyn Fn(u8)>) -> u16;
}

fn main() {}
