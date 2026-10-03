//! ADR-058: a function or method with a type parameter lists the types it crosses for; without
//! the list there is nothing to register, and the text says what to write. A constructor is never
//! generic.
#![allow(unused)]

use undra::prelude::*;
use undra_macros as k;

#[k::api]
pub struct Todo {
    pub title: String,
}

#[k::api]
pub fn newest<T: Clone>(rows: Vec<T>) -> Option<T> {
    rows.into_iter().next()
}

pub struct Library;

#[k::api]
impl Library {
    pub fn new() -> Self {
        Library
    }

    pub fn pinned<T: Default>(&self) -> Vec<T> {
        Vec::new()
    }
}

pub struct Mailbox;

#[k::api]
impl Mailbox {
    pub fn create<T>(seed: T) -> Self {
        Mailbox
    }
}

fn main() {}
