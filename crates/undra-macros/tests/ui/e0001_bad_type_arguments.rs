//! ADR-042: the arguments of an instantiation are checked where the template puts them, with the
//! rules of the position they land in: `Vec<()>` is E0001, `Option<Option<_>>` is E0063, an object
//! is not a value (E0064), and a type that is not an Undra type fails its identity check (E0061).
#![allow(unused)]

use undra_macros as k;

#[k::api(generic)]
pub struct Page<T> {
    pub items: Vec<T>,
}

#[k::api(generic)]
pub struct Maybe<T> {
    pub value: Option<T>,
}

pub struct Calculator;

#[k::api]
impl Calculator {
    pub fn new() -> Self {
        Calculator
    }

    pub fn answer(&self) -> u32 {
        42
    }
}

pub struct Plain;

#[k::api]
pub type Units = Page<()>;

#[k::api]
pub type Borrowed = Page<&'static str>;

#[k::api]
pub type Nested = Maybe<Option<u8>>;

#[k::api]
pub type Objects = Page<Calculator>;

#[k::api]
pub type NotUndra = Page<Plain>;

fn main() {}
