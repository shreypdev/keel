//! ADR-058: a generic function lists the types it crosses the boundary for, `generic(T = [Todo,
//! Note])`. The list is read at compile time: what it may not say is E0072, each time with the
//! fix. One instantiation is one function of the schema, so a list is a list of named value types.
#![allow(unused)]

use std::sync::Arc;

use undra_macros as k;

#[k::api]
pub struct Todo {
    pub title: String,
}

#[k::api]
pub struct Tag {
    pub name: String,
}

/// A list of a parameter the function does not have.
#[k::api(generic(U = [Todo]))]
pub fn wrong_parameter<T>(rows: Vec<T>) -> Option<T> {
    rows.into_iter().next()
}

/// A list on a function with no type parameter at all.
#[k::api(generic(T = [Todo]))]
pub fn no_parameter(rows: Vec<Todo>) -> Option<Todo> {
    rows.into_iter().next()
}

/// An empty list: nothing would cross.
#[k::api(generic(T = []))]
pub fn empty<T>(rows: Vec<T>) -> Option<T> {
    rows.into_iter().next()
}

/// One type twice: two functions would share one name.
#[k::api(generic(T = [Todo, Todo]))]
pub fn repeated<T>(rows: Vec<T>) -> Option<T> {
    rows.into_iter().next()
}

/// A container, a scalar, `String` and a shared pointer are not named value types.
#[k::api(generic(T = [Vec<Todo>, u32, String, Arc<Todo>]))]
pub fn unnamed<T>(rows: Vec<T>) -> Option<T> {
    rows.into_iter().next()
}

/// Two type parameters would need a list of pairs.
#[k::api(generic(A = [Todo], B = [Tag]))]
pub fn link<A, B>(a: A, b: B) -> u32 {
    0
}

fn main() {}
