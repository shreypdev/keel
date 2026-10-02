//! `testkit/conformance/fakes.json` is what the Rust fakes answer; the three platform kits replay
//! it against theirs. This test keeps the file honest: it fails when the file is not what
//! `conformance::generate()` writes. `UNDRA_BLESS=1 cargo test -p undra-testkit --test
//! conformance` rewrites it.

use std::path::PathBuf;

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testkit/conformance/fakes.json")
}

#[test]
fn the_checked_in_file_is_what_the_rust_fakes_answer() {
    let generated = undra_testkit::conformance::generate();
    if std::env::var_os("UNDRA_BLESS").is_some() {
        std::fs::write(path(), &generated).expect("write the conformance file");
        return;
    }
    let on_disk = std::fs::read_to_string(path()).expect(
        "testkit/conformance/fakes.json is missing: UNDRA_BLESS=1 cargo test -p undra-testkit --test conformance",
    );
    assert!(
        on_disk == generated,
        "testkit/conformance/fakes.json is stale: run UNDRA_BLESS=1 cargo test -p undra-testkit --test conformance"
    );
}
