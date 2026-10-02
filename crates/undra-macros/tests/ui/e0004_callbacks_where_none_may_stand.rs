//! E0004 (ADR-041): a callback where none may stand, a closure parameter, a trait that is not a
//! callback interface.
#![allow(unused)]

use std::sync::Arc;

use undra_macros as k;

#[k::callback]
pub trait Listener {
    fn changed(&self, value: u32);
}

#[k::port]
pub trait Platform {
    fn ping(&self);
}

#[k::api]
pub struct Record {
    pub listener: Arc<dyn Listener>,
}

pub struct Service;

#[k::api]
impl Service {
    pub fn new() -> Self {
        Service
    }

    pub fn returns_one(&self) -> Arc<dyn Listener> {
        unimplemented!()
    }

    pub fn many(&self, listeners: Vec<Arc<dyn Listener>>) {}

    pub fn closure(&self, f: Arc<dyn Fn(u32) + Send + Sync>) {}

    pub fn not_a_callback(&self, p: Arc<dyn Platform>) {}
}

#[k::port]
pub trait Backend {
    fn call(&self, listener: Arc<dyn Listener>);
}

fn main() {}
