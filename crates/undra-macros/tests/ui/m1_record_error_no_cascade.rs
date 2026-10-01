//! M1 (records and errors): a record, enum or error that fails to expand keeps the members its
//! users rely on, so the one real error is not followed by "cannot cross the boundary" and
//! identity-check errors at every use.
#![allow(unused)]

use undra_macros as k;

#[k::api]
pub struct Todo {
    pub n: usize,
}

#[k::error]
pub enum Failure {
    #[error("failed")]
    Failed,
    Unmessaged,
}

#[k::api]
pub fn get(id: u32) -> Result<Vec<Todo>, Failure> {
    Ok(Vec::new())
}

pub struct Holder;

#[k::api]
impl Holder {
    pub fn new() -> Self {
        Holder
    }

    pub fn find(&self, todo: Todo) -> Option<Todo> {
        Some(todo)
    }
}

fn main() {
    let _ = Failure::Failed.to_string();
}
