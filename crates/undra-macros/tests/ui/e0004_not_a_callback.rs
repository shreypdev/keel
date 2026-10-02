//! E0004 (ADR-041): `Arc<dyn Trait>` of a trait that is not a `#[undra::callback]`.
#![allow(unused)]

use std::sync::Arc;

use undra_macros as k;

#[k::port]
pub trait Platform {
    fn ping(&self);
}

pub trait Plain: Send + Sync {
    fn go(&self);
}

pub struct Service;

#[k::api]
impl Service {
    pub fn new() -> Self {
        Service
    }

    pub fn takes_a_port(&self, p: Arc<dyn Platform>) {}

    pub fn takes_a_plain_trait(&self, p: Arc<dyn Plain>) {}
}

fn main() {}
