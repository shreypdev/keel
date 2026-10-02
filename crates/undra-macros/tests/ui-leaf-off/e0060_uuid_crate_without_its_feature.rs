//! ADR-042 decision 4.3: a bare `Uuid` can be `undra::Uuid` or `uuid::Uuid`, so the spelling alone
//! cannot say the feature is missing; the membership assertion does (E0060, naming the features),
//! next to the two codec errors of a type that is not in the schema's vocabulary.
#![allow(unused)]

use undra_macros as k;
use uuid::Uuid;

#[k::api]
pub struct Account {
    pub id: Uuid,
    pub name: String,
}

fn main() {}
