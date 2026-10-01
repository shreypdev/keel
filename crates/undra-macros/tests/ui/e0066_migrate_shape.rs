//! E0066: a `#[undra::migrate]` hook names exactly one target and has the target's shape.
#![allow(unused)]

use undra::prelude::*;

#[undra::api]
pub struct Todo {
    pub title: String,
}

// No target.
#[undra::migrate]
fn nothing(old: &DynValue) -> Result<Todo, MigrateError> {
    unimplemented!()
}

// Two targets.
#[undra::migrate(ty = "Todo", mutation = "add_todo")]
fn both(old: &DynValue) -> Result<Todo, MigrateError> {
    unimplemented!()
}

// A signal without its store.
#[undra::migrate(signal = "age")]
fn orphan(old: Option<&DynValue>) -> Result<f32, MigrateError> {
    unimplemented!()
}

// The parameter of a signal hook is `Option<&DynValue>`.
#[undra::migrate(store = "Profile", signal = "age")]
fn wrong_parameter(old: &DynValue) -> Result<f32, MigrateError> {
    unimplemented!()
}

// A mutation hook returns the new input as a `DynRecord`.
#[undra::migrate(mutation = "add_todo")]
fn wrong_return(old: &DynRecord) -> Result<Todo, MigrateError> {
    unimplemented!()
}

// Not a fingerprint.
#[undra::migrate(ty = "Todo", from = "not-hex")]
fn bad_from(old: &DynValue) -> Result<Todo, MigrateError> {
    unimplemented!()
}

// An async hook.
#[undra::migrate(ty = "Todo")]
async fn later(old: &DynValue) -> Result<Todo, MigrateError> {
    unimplemented!()
}

fn main() {}
