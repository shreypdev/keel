//! The boundary fixture core: `tests/common/core.rs` linked with `keel-ffi` into one library.
//! `keel-ffi` is referenced so its `keel_*` exports are linked into this cdylib/staticlib.

#[path = "../../common/core.rs"]
mod test_core;

pub use keel_ffi::*;
