//! Compile-pass tests for objects, stores, ports and queries against the real `keel` facade.
//!
//! `tests/ui-runtime/*.rs` are written the way a user writes a core: `use keel::prelude::*;`
//! and `#[keel::api]`. They must compile against the real `keel-runtime` and `keel-signals`
//! (the `keel` dev-dependency), which is what checks that every name the macros emit exists
//! and has the shape the macros assume.

#[test]
fn generated_code_compiles_against_the_runtime() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui-runtime/*.rs");
}
