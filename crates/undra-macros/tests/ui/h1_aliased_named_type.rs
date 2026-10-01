//! H1: the schema names a type by the spelling it was written with. A renamed import or a type
//! alias makes the name point at a different type than the wire carries: a platform that sends a
//! `Todo` would be decoded as an `Item`.
#![allow(unused)]

use undra_macros as k;

mod v2 {
    #[undra_macros::api]
    pub struct Item {
        pub id: u32,
    }
}

mod legacy {
    use crate::v2::Item as Todo;

    #[undra_macros::api]
    pub fn legacy_echo(t: Todo) -> Todo {
        t
    }
}

type Entry = v2::Item;

#[k::api]
pub struct Wrapper {
    pub inner: Entry,
}

fn main() {}
