//! ADR-058: the signatures of a generic object are checked once, at the template, with the type
//! parameters left as they are: a borrowed `&str` is one error at the template, not one for each
//! instantiation.
#![allow(unused)]

use undra_macros as k;

#[k::api]
#[derive(Clone)]
pub struct Todo {
    pub id: u32,
}

#[k::api]
#[derive(Clone)]
pub struct Note {
    pub id: u32,
}

pub struct Cache<T>(Vec<T>);

#[k::api(generic)]
impl<T: Send + Sync + 'static> Cache<T> {
    pub fn new() -> Self {
        Cache(Vec::new())
    }

    pub fn rename(&self, name: &str) {}

    pub fn put(&self, item: T) {}
}

#[k::api]
pub type TodoCache = Cache<Todo>;

#[k::api]
pub type NoteCache = Cache<Note>;

fn main() {}
