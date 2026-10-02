//! E0061 (ADR-040): an object written under another name than it is declared with, and
//! `Arc<T>` of something that is not an object at all.
#![allow(unused)]

use std::sync::Arc;

use undra_macros as k;

pub struct Child;

#[k::api]
impl Child {
    pub fn new() -> Self {
        Child
    }
}

type Kid = Child;

pub struct NotAnObject;

pub struct Parent;

#[k::api]
impl Parent {
    pub fn new() -> Self {
        Parent
    }

    pub fn aliased(&self) -> Arc<Kid> {
        Arc::new(Child)
    }

    pub fn plain(&self, p: Arc<NotAnObject>) {}
}

fn main() {}
