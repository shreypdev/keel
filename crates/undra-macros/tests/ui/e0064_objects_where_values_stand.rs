//! E0064 (ADR-040): an object where a value stands (a record field, a map value, a query value,
//! a port argument, nested deeper than `Option` or `Vec`), and a record written as an object.
#![allow(unused)]

use std::collections::HashMap;
use std::sync::Arc;

use undra::runtime::Ctx;
use undra_macros as k;

pub struct Child;

#[k::api]
impl Child {
    pub fn new() -> Self {
        Child
    }
}

#[k::api]
pub struct Holder {
    pub child: Arc<Child>,
}

pub struct Parent;

#[k::api]
impl Parent {
    pub fn new() -> Self {
        Parent
    }

    pub fn by_name(&self) -> HashMap<String, Arc<Child>> {
        HashMap::new()
    }

    pub fn nested(&self) -> Vec<Option<Arc<Child>>> {
        Vec::new()
    }

    pub fn borrowed_list(&self, children: Vec<&Child>) {}

    pub fn a_record_as_an_object(&self, value: Arc<Plain>) {}
}

#[k::api]
pub struct Plain {
    pub id: u32,
}

#[k::error]
pub enum Oops {
    #[error("oops")]
    Oops,
}

#[k::query(key = "child")]
pub async fn child_query(ctx: &Ctx) -> Result<Arc<Child>, Oops> {
    Ok(Arc::new(Child))
}

#[k::port]
pub trait Backend {
    async fn open(&self, child: Arc<Child>) -> Result<(), Oops>;
}

fn main() {}
