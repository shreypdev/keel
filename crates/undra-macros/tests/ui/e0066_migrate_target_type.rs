//! E0066: a `ty = ".."` hook returns the type it targets, checked in the user's crate.
#![allow(unused)]

use undra::prelude::*;

#[undra::api]
pub struct Todo {
    pub title: String,
}

#[undra::api]
pub struct Note {
    pub text: String,
}

/// The target says `Todo`, the function builds a `Note`.
#[undra::migrate(ty = "Todo")]
fn crossed(old: &DynValue) -> Result<Note, MigrateError> {
    unimplemented!()
}

/// A plain struct is not a type of the schema.
pub struct Plain;

#[undra::migrate(ty = "Plain")]
fn not_undra(old: &DynValue) -> Result<Plain, MigrateError> {
    unimplemented!()
}

fn main() {}
