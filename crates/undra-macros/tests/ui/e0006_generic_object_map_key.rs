//! ADR-058: the type arguments of an alias are whatever the positions they land in accept, checked
//! exactly as if the instantiation had been written by hand: a float is not a map key (E0006), and
//! a row type without the key's field is E0008, both at the alias.
#![allow(unused)]

use std::collections::HashMap;

use undra::prelude::*;
use undra_macros as k;

pub struct Directory<K, V>(HashMap<K, V>);

#[k::api(generic)]
impl<K: Send + Sync + 'static, V: Send + Sync + 'static> Directory<K, V> {
    pub fn new() -> Self {
        loop {}
    }

    pub fn all(&self) -> HashMap<K, V> {
        loop {}
    }
}

#[k::api]
pub type Floats = Directory<f64, String>;

#[k::api]
#[derive(Clone)]
pub struct Tag {
    pub name: String,
}

#[k::store(generic)]
pub struct Selection<T> {
    #[undra(key = "id")]
    rows: Signal<Vec<T>>,
}

#[k::api(store, generic)]
impl<T: SignalValue> Selection<T> {
    pub fn new(ctx: Ctx) -> Self {
        Selection {
            rows: Signal::new(Vec::new()),
        }
    }
}

#[k::api]
pub type TagSelection = Selection<Tag>;

fn main() {}
