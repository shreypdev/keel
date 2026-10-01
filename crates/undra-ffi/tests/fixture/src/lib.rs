//! The boundary fixture core: `tests/common/core.rs` linked with `undra-ffi` into one library.
//! `undra-ffi` is referenced so its `undra_*` exports are linked into this cdylib/staticlib.

#[path = "../../common/core.rs"]
mod test_core;

pub use undra_ffi::*;
