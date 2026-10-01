//! Compile-pass tests for objects, stores, ports and queries against the real `undra` facade.
//!
//! `tests/ui-runtime/*.rs` are written the way a user writes a core: `use undra::prelude::*;`
//! and `#[undra::api]`. They must compile against the real `undra-runtime` and `undra-signals`
//! (the `undra` dev-dependency), which is what checks that every name the macros emit exists
//! and has the shape the macros assume.

#[test]
fn generated_code_compiles_against_the_runtime() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui-runtime/*.rs");
}
