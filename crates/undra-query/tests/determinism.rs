//! Constitution R12: the core is deterministic. The query layer reads no wall clock, uses no
//! random source of its own and starts no thread; time comes from the `Clock` port, randomness
//! from `Rng`, delays from `Ctx::sleep`. This test reads the crate's sources and fails if one of
//! the forbidden things appears (the in-tree equivalent of a `clippy.toml` disallowed-methods
//! list, which cannot express "except inside the runtime").

use std::path::PathBuf;

const FORBIDDEN: &[(&str, &str)] = &[
    ("SystemTime", "the wall clock: use the Clock port"),
    ("Instant::now", "the monotonic clock: use the Clock port"),
    ("thread::spawn", "threads belong to undra-runtime"),
    (
        "thread::sleep",
        "sleeping belongs to the Timer port (ctx.sleep)",
    ),
    ("rand::", "randomness: use the Rng port"),
    ("getrandom", "randomness: use the Rng port"),
    ("println!", "logging goes through the Log port"),
    ("eprintln!", "logging goes through the Log port"),
    ("unsafe ", "unsafe lives in undra-ffi only"),
];

#[test]
fn the_sources_use_only_the_ports_for_time_randomness_and_threads() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0;
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        // The unit tests and doc examples may do what they like; the rule is about the code.
        let code = text.split("#[cfg(test)]").next().unwrap();
        for (line_no, line) in code.lines().enumerate() {
            let line = line.trim_start();
            if line.starts_with("//") {
                continue;
            }
            for (needle, why) in FORBIDDEN {
                assert!(
                    !line.contains(needle),
                    "{}:{}: `{needle}` ({why})\n    {line}",
                    path.display(),
                    line_no + 1
                );
            }
        }
        checked += 1;
    }
    assert!(checked >= 10, "the scan found only {checked} source files");
}

#[test]
fn the_crate_forbids_unsafe_code() {
    let lib = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .unwrap();
    assert!(lib.contains("#![forbid(unsafe_code)]"));
    assert!(lib.contains("#![deny(missing_docs)]"));
}
