#![allow(unused)]

use keel_macros as k;

#[k::error]
pub enum TodoError {
    #[error("title cannot be empty")]
    EmptyTitle,
    NotFound(u32),
    #[error("todo {1} not found")]
    Missing(u32),
    #[error(transparent)]
    Both(u32, u32),
}

fn main() {}
