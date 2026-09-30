//! M3: the error side of a `Result` must be a `#[keel::error]` enum. A `String` is rejected
//! when the macro runs; a record, or a type that is not a Keel type, by an assertion on the
//! error type itself.
#![allow(unused)]

use keel_macros as k;

#[k::api]
pub struct Problem {
    pub code: u32,
}

#[k::api]
pub fn by_string(a: u8) -> Result<u8, String> {
    Ok(a)
}

#[k::api]
pub fn by_record(a: u8) -> Result<u8, Problem> {
    Ok(a)
}

fn main() {}
