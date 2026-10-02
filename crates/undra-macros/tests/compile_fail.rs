//! Compile-fail tests for the diagnostics of SPEC section 12.
//!
//! Each file in `tests/ui/` triggers one diagnostic; the expected compiler output is in the
//! `.stderr` next to it. After an intended change of a message, regenerate with
//! `TRYBUILD=overwrite cargo test -p undra-macros --test compile_fail` and review the diff.

#[test]
fn diagnostics_render_as_documented() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}

/// The types of the opt-in leaf features without their feature (ADR-042): the macros mirror the
/// features of `undra`, so these only make sense while they are off. Run with
/// `cargo test -p undra-macros --test compile_fail`; with a feature on, its case is skipped.
#[test]
#[cfg(not(any(
    feature = "uuid",
    feature = "chrono",
    feature = "time",
    feature = "rust_decimal",
    feature = "bytes"
)))]
fn leaf_types_without_their_feature_name_it() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui-leaf-off/*.rs");
}
