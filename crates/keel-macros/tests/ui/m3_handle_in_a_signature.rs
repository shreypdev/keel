//! M3: `Handle` has no schema representation; an object crosses the boundary through its
//! constructors, never as a value in a signature.
#![allow(unused)]

use keel::prelude::Handle;
use keel_macros as k;

#[k::api]
pub fn close(handle: Handle) {}

#[k::api]
pub struct Holder {
    pub inner: Handle,
}

fn main() {}
