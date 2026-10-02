//! ADR-058: what goes wrong for one listed type and not for another is reported under the name of
//! the instantiation, `newest<Todo>`: the tokens of a signature are the same for every type of the
//! list, so the message says which one it is about.
#![allow(unused)]

use undra_macros as k;

#[k::api]
pub struct Todo {
    pub title: String,
}

#[k::api]
pub struct Note {
    pub body: String,
}

#[k::error]
pub enum Failure {
    #[error("failed")]
    Failed,
}

/// `T` is the error type of the result: a record is not an error.
#[k::api(generic(T = [Todo, Failure]))]
pub fn fails_with<T: 'static>() -> Result<u32, T> {
    Ok(0)
}

/// A mistake that does not depend on the type is reported once, at the function, not once for
/// each type of the list.
#[k::api(generic(T = [Todo, Note]))]
pub fn borrows<T: 'static>(name: &str, rows: Vec<T>) -> u32 {
    0
}

fn main() {}
