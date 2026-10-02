#![deny(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![doc = include_str!("../README.md")]
//!
//! # Where things live
//!
//! | Surface | Module | Built when |
//! |---|---|---|
//! | C ABI v2 (SPEC 6): one [`UndraApi`] table per core, exported as `<namespace>_undra_api` by [`export_core!`] | `table`, `native` | not `wasm` |
//! | JNI shim (SPEC 6.1): `JNI_OnLoad` registering the natives on the core's own class ([`export_core!`]'s `jni_class`) | `jni_shim` | feature `jni`, not `wasm` |
//! | wasm ABI (SPEC 7): `undra_alloc`, `undra_poll`, the `"undra"` imports | `wasm` | `wasm` |
//!
//! All three are thin shells over [`undra_runtime::Runtime`]: the Rust semantics live in one
//! private layer (`api`), so a behaviour is the same whichever door the host uses. This is the
//! only crate of the workspace that contains `unsafe` (constitution R2); every block carries a
//! `// SAFETY:` comment, which `clippy::undocumented_unsafe_blocks` enforces.
//!
//! On native targets this crate exports **no symbol of its own** (ADR-044): the crate built as the
//! core's library calls [`export_core!`] once, which exports the namespaced table entry and the JNI
//! entry points. Two cores linked into one process therefore never share a symbol, and each image
//! carries its own runtime, registries and threads.

#[cfg(all(panic = "abort", not(target_family = "wasm")))]
compile_error!(
    "undra-ffi needs `panic = \"unwind\"` on native targets: nothing may escape the boundary as an \
     abort (constitution R6), and a panic in a dispatched call is answered with a status 2 reply \
     by catching it. Only the wasm profile may abort (SPEC 7); see `[profile.release-wasm]` in \
     the workspace Cargo.toml."
);

mod api;
mod buf;
#[cfg(not(target_family = "wasm"))]
mod frames;
mod guard;
mod table;

/// What the macros of this crate expand to; not API.
#[doc(hidden)]
pub mod __private {
    pub use undra_runtime::CoreIdentity;
    pub use undra_runtime::inventory;
}

pub use api::{ABI_VERSION, WASM_ABI_VERSION, init_code, restore_code};
pub use buf::UndraBuf;
#[doc(hidden)]
pub use table::__namespace;
#[cfg(not(target_family = "wasm"))]
pub use table::UndraApi;
pub use table::{MAX_NAMESPACE_LEN, is_valid_namespace};

#[cfg(not(target_family = "wasm"))]
mod registry;
#[cfg(not(target_family = "wasm"))]
mod session;

#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(not(target_family = "wasm"))]
pub use native::{UndraChangesetCb, UndraPortCb, UndraReplyCb, UndraStreamCb};

#[cfg(all(feature = "jni", not(target_family = "wasm")))]
mod jni_shim;
#[cfg(all(feature = "jni", not(target_family = "wasm")))]
#[doc(hidden)]
pub use jni_shim::{on_load as __jni_on_load, on_unload as __jni_on_unload};

#[cfg(any(target_family = "wasm", test))]
mod builtin;
#[cfg(any(target_family = "wasm", test))]
mod replies;

#[cfg(target_family = "wasm")]
pub mod wasm;
