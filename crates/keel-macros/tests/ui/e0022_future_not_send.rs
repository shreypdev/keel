#![allow(unused)]

use std::rc::Rc;

use keel_macros as k;

pub struct Worker;

#[k::api]
impl Worker {
    pub fn new() -> Self {
        Worker
    }

    pub async fn run(&self) -> u32 {
        let local = Rc::new(1_u32);
        std::future::ready(()).await;
        *local
    }
}

fn main() {}
