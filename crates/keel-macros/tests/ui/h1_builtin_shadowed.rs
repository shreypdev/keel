//! H1: a user type spelled like a built-in Keel type must not pass for it. The schema would say
//! `bytes` while the generated code moves the user's record (and a `mime` field nobody declared).
#![allow(unused)]

use keel_macros as k;

/// The user's own record, unrelated to `keel::wire::Bytes`.
#[k::api]
pub struct Bytes {
    pub data: Vec<u8>,
    pub mime: String,
}

#[k::api]
pub struct Doc {
    pub payload: Bytes,
    pub n: u32,
}

/// Not a Keel type at all.
pub struct Uuid(pub u128);

#[k::api]
pub fn lookup(id: Uuid) -> String {
    String::new()
}

fn main() {}
