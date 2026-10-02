//! H1 with the wire leaves (ADR-042): a type of your own spelled like `Timestamp`, `Duration`,
//! `Decimal` or `Bytes` is not the leaf, and the assertion is a membership one: it names the
//! leaves and, for the types of other crates, the features.
#![allow(unused)]

use undra_macros as k;

pub struct Timestamp(pub u64);
pub struct Decimal(pub String);

#[k::api]
pub struct Reading {
    pub at: Timestamp,
    pub amount: Decimal,
}

fn main() {}
