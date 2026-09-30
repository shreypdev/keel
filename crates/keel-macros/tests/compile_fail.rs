//! Compile-fail tests for the diagnostics of SPEC section 12.
//!
//! Each file in `tests/ui/` triggers one diagnostic; the expected compiler output is in the
//! `.stderr` next to it. After an intended change of a message, regenerate with
//! `TRYBUILD=overwrite cargo test -p keel-macros --test compile_fail` and review the diff.

#[test]
fn diagnostics_render_as_documented() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}
