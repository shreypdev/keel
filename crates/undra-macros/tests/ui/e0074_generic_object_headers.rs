//! ADR-058: the impl block of a generic object is for the type with its own type parameters, each
//! exactly once (E0074), and its methods take no type parameters of their own: an instantiation is
//! one class on each platform, and a second level of generics would multiply every one of them.
#![allow(unused)]

use undra_macros as k;

#[k::api]
pub struct Todo {
    pub title: String,
}

pub struct Cache<T>(Vec<T>);

/// The block is for `Cache<Vec<T>>`, not for `Cache<T>`.
#[k::api(generic)]
impl<T> Cache<Vec<T>> {
    pub fn len(&self) -> u32 {
        0
    }
}

pub struct Pair<A, B>(A, B);

/// A concrete argument next to the parameter.
#[k::api(generic)]
impl<A> Pair<A, Todo> {
    pub fn first(&self) -> u32 {
        0
    }
}

pub struct Twin<A, B>(A, B);

/// A parameter applied twice and another not at all.
#[k::api(generic)]
impl<A, B> Twin<A, A> {
    pub fn second(&self) -> u32 {
        0
    }
}

pub struct Box2<T>(T);

#[k::api(generic)]
impl<T: Clone + Default> Box2<T> {
    pub fn new() -> Self {
        Box2(T::default())
    }

    /// A list on a method of a generic object.
    #[undra(generic(U = [Todo]))]
    pub fn convert<U: Default>(&self) -> U {
        U::default()
    }

    /// A type parameter on a method, with no list.
    pub fn map<U: Default>(&self) -> U {
        U::default()
    }
}

fn main() {}
