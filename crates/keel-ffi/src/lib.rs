#![deny(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![doc = include_str!("../README.md")]
//!
//! # Where things live
//!
//! | Surface | Module | Built when |
//! |---|---|---|
//! | C ABI (SPEC 6): `keel_init`, `keel_call`, ... | [`native`] | not `wasm` |
//! | JNI shim (SPEC 6.1): `Java_dev_keel_runtime_KeelNative_*`, `JNI_OnLoad` | `jni_shim` | feature `jni`, not `wasm` |
//! | wasm ABI (SPEC 7): `keel_alloc`, `keel_poll`, the `"keel"` imports | `wasm` | `wasm` |
//!
//! All three are thin shells over [`keel_runtime::Runtime`]: the Rust semantics live in one
//! private layer (`api`), so a behaviour is the same whichever door the host uses. This is the
//! only crate of the workspace that contains `unsafe` (constitution R2); every block carries a
//! `// SAFETY:` comment, which `clippy::undocumented_unsafe_blocks` enforces.

mod api;
mod buf;
mod guard;

pub use api::{ABI_VERSION, init_code, restore_code};
pub use buf::KeelBuf;

#[cfg(not(target_family = "wasm"))]
mod session;

#[cfg(not(target_family = "wasm"))]
pub mod native;
#[cfg(not(target_family = "wasm"))]
pub use native::*;

#[cfg(all(feature = "jni", not(target_family = "wasm")))]
mod jni_shim;

#[cfg(target_family = "wasm")]
pub mod wasm;
