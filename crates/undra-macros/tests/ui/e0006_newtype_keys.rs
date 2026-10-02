//! ADR-042: a newtype is a map key exactly when the type it wraps is, and the compiler decides:
//! a record and a newtype of a decimal are E0006; `UserId` and `Sku` are fine.
#![allow(unused)]

use std::collections::{BTreeMap, HashMap};

use undra::prelude::*;
use undra_macros as k;

#[k::api]
#[derive(PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UserId(pub Uuid);

#[k::api]
#[derive(PartialEq, Eq, Hash)]
pub struct Price(pub Decimal);

#[k::api]
#[derive(PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Owner(pub UserId);

#[k::api]
#[derive(PartialEq, Eq, Hash)]
pub struct Loose {
    pub id: u32,
}

#[k::api]
pub struct Ledger {
    pub fine: HashMap<UserId, u32>,
    pub nested: BTreeMap<Owner, String>,
    pub by_price: HashMap<Price, u32>,
    pub by_record: HashMap<Loose, u32>,
}

fn main() {}
