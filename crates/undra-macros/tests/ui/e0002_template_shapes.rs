//! ADR-042: a template takes type parameters only (lifetimes are E0003), and every instantiation
//! names all of them.
#![allow(unused)]

use undra_macros as k;

#[k::api(generic)]
pub struct WithWhere<T>
where
    T: Clone,
{
    pub items: Vec<T>,
}

#[k::api(generic)]
pub struct WithConst<T, const N: usize> {
    pub items: Vec<T>,
}

#[k::api(generic)]
pub struct WithDefault<T = String> {
    pub items: Vec<T>,
}

#[k::api(generic)]
pub struct WithLifetime<'a, T> {
    pub items: Vec<T>,
    pub name: &'a str,
}

#[k::api(generic)]
pub struct NotGeneric {
    pub items: Vec<u8>,
}

#[k::api(generic)]
pub struct Assoc<T: Iterator> {
    pub first: T::Item,
}

#[k::api(generic)]
pub fn not_a_type<T>(items: Vec<T>) {}

#[k::api(generic)]
pub struct Pair<A, B> {
    pub a: A,
    pub b: B,
}

#[k::api]
pub type Both = Pair<u8>;

#[k::api]
pub type Neither = Pair<u8, u8, u8>;

fn main() {}
