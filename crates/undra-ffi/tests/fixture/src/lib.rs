//! The boundary fixture core: `tests/common/core.rs` linked with `undra-ffi` into one library.
//!
//! It exports what any core exports (ADR-044): `undra_fixture_undra_api()`, the C ABI table, and with
//! the `jni` feature `JNI_OnLoad`, which registers the natives on `dev.undra.fixture.UndraCoreNative`
//! (the JNI end-to-end run and the Kotlin runtime's native tests declare that class). On wasm the
//! exports are the wasm ABI's own (SPEC 7).

#[path = "../../common/core.rs"]
mod test_core;

undra_ffi::export_core!(undra_fixture, jni_class = "dev/undra/fixture/UndraCoreNative");
