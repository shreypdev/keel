//! ADR-058: a listed type is a value type. The macro cannot see that a plain name is an object, so
//! the compiler says so, with the same code and the same fix.
#![allow(unused)]

use undra_macros as k;

pub struct Mailbox;

#[k::api]
impl Mailbox {
    pub fn new() -> Self {
        Mailbox
    }
}

#[k::api(generic(T = [Mailbox]))]
pub fn list_an_object<T>(rows: Vec<T>) -> u32 {
    rows.len() as u32
}

fn main() {}
