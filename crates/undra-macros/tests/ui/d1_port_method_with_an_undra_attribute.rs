//! D1: a query, mutation or function written inside a port trait. A port is what the platform
//! implements; queries and functions are free functions that call it.
#![allow(unused)]

use undra_macros as k;

#[k::port]
pub trait Weather {
    #[undra::query(key = "forecast")]
    async fn forecast(&self) -> String;

    #[undra::api]
    fn now(&self) -> String;
}

fn main() {}
