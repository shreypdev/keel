//! Compile-pass tests for objects, stores, ports and queries against the `keel` facade.
//!
//! `tests/ui-runtime/*.rs` are written the way a user writes a core: `use keel::prelude::*;`
//! and `#[keel::api]`. They must compile against the *real* `keel-runtime` and
//! `keel-signals`. Until those land, the `keel` dev-dependency of this crate is the
//! test facade (`tests/facade`), a naive stand-in for exactly the names the macros use, so this
//! test is `#[ignore]`d by default (run it with `-- --ignored`; it passes against the facade).
//!
//! To turn it on for real: point the `keel` dev-dependency at `../keel`, delete the `ignore`
//! attribute below, and fix whatever the integration reveals (the names the macros use are
//! listed in the keel-macros report).

#[test]
#[ignore = "needs the real keel-runtime and keel-signals behind the `keel` dev-dependency"]
fn generated_code_compiles_against_the_runtime() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui-runtime/*.rs");
}
