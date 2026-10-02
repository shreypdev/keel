//! The fixtures under `testkit/fixtures/` that the three platform kits read: they pin the format
//! (the canonical writer, byte for byte) and the seed document. A kit that reads and writes them
//! unchanged speaks the same format as this crate.
//!
//! `UNDRA_BLESS=1 cargo test -p undra-testkit --test fixtures` rewrites the generated ones.

use std::path::PathBuf;

use undra_testkit::{Harness, Recording, Seed, every_kind};

fn path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testkit/fixtures")
        .join(name)
}

#[test]
fn the_all_kinds_fixture_pins_the_canonical_writer() {
    let text = every_kind().to_json();
    if std::env::var_os("UNDRA_BLESS").is_some() {
        std::fs::write(path("recording-all-kinds.json"), &text).expect("write");
        return;
    }
    let on_disk = std::fs::read_to_string(path("recording-all-kinds.json"))
        .expect("testkit/fixtures/recording-all-kinds.json is missing: UNDRA_BLESS=1 cargo test -p undra-testkit --test fixtures");
    assert!(
        on_disk == text,
        "recording-all-kinds.json is stale: UNDRA_BLESS=1 cargo test -p undra-testkit --test fixtures"
    );
    // Reading and writing it again is the identity.
    assert_eq!(Recording::from_json(&on_disk).unwrap().to_json(), on_disk);
}

#[test]
fn every_checked_in_recording_reads_and_rewrites_to_itself() {
    for name in [
        "session-todos.json",
        "ports-remote-todos.json",
        "recording-all-kinds.json",
    ] {
        let text =
            std::fs::read_to_string(path(name)).unwrap_or_else(|_| panic!("{name} is missing"));
        let recording = Recording::from_json(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(recording.to_json(), text, "{name} is canonical");
    }
}

#[test]
fn the_example_seed_reads_and_seeds_a_harness() {
    let text = std::fs::read_to_string(path("seed.json")).expect("testkit/fixtures/seed.json");
    let seed = Seed::from_json(&text).expect("the example seed reads");
    assert_eq!(seed.now_ms, Some(1_700_000_000_000));
    let h = Harness::from_seed(&seed).expect("and applies");
    assert_eq!(h.fakes().kv.value("greeting"), Some(b"hello".to_vec()));
    assert_eq!(h.fakes().kv.value("blob"), Some(vec![0, 255]));
    assert_eq!(
        h.fakes().secure_store.value("token"),
        Some(b"t-123".to_vec())
    );
    assert_eq!(
        h.fakes().fs.contents("notes/a.txt"),
        Some(b"hello".to_vec())
    );
}
