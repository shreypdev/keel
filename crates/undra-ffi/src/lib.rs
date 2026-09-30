#![deny(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![doc = include_str!("../README.md")]
//!
//! # Where things live
//!
//! | Surface | Module | Built when |
//! |---|---|---|
//! | C ABI (SPEC 6): `undra_init`, `undra_call`, ... | `native` | not `wasm` |
//! | JNI shim (SPEC 6.1): `Java_dev_undra_runtime_UndraNative_*`, `JNI_OnLoad` | `jni_shim` | feature `jni`, not `wasm` |
//! | wasm ABI (SPEC 7): `undra_alloc`, `undra_poll`, the `"undra"` imports | `wasm` | `wasm` |
//!
//! All three are thin shells over [`undra_runtime::Runtime`]: the Rust semantics live in one
//! private layer (`api`), so a behaviour is the same whichever door the host uses. This is the
//! only crate of the workspace that contains `unsafe` (constitution R2); every block carries a
//! `// SAFETY:` comment, which `clippy::undocumented_unsafe_blocks` enforces.

#[cfg(all(panic = "abort", not(target_family = "wasm")))]
compile_error!(
    "undra-ffi needs `panic = \"unwind\"` on native targets: nothing may escape the boundary as an \
     abort (constitution R6), and a panic in a dispatched call is answered with a status 2 reply \
     by catching it. Only the wasm profile may abort (SPEC 7); see `[profile.release-wasm]` in \
     the workspace Cargo.toml."
);

mod api;
mod buf;
mod guard;

pub use api::{ABI_VERSION, init_code, restore_code};
pub use buf::UndraBuf;

#[cfg(not(target_family = "wasm"))]
mod registry;
#[cfg(not(target_family = "wasm"))]
mod session;

#[cfg(not(target_family = "wasm"))]
pub mod native;
#[cfg(not(target_family = "wasm"))]
pub use native::*;

#[cfg(all(feature = "jni", not(target_family = "wasm")))]
mod jni_shim;

#[cfg(any(target_family = "wasm", test))]
mod builtin;
#[cfg(any(target_family = "wasm", test))]
mod replies;

#[cfg(target_family = "wasm")]
pub mod wasm;
