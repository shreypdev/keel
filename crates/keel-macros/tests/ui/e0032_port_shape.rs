#![allow(unused)]

use keel_macros as k;

#[k::port(sync)]
pub trait Clock {
    async fn now_ms(&self) -> i64;
}

#[k::port]
pub trait Kv {
    type Error;

    fn get(&self, key: String) -> Option<String>;
}

fn main() {}
