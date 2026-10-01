//! `undra bindgen --docs` reads the doc comments from the built core library (SPEC 2.3, 13), and
//! the bindings it writes are exactly the ones the dev runner's schema produces.
//!
//! Until `undra_schema_json` carried docs, `--docs` ran the dev runner (`--print-schema`) to get
//! them. Both routes now describe the same registrations, so this test builds both for the
//! playground and compares what comes out, byte for byte, and pins the playground's schema hash
//! across them: the hash covers neither docs nor labels, so neither route may move it.
//!
//! `#[ignore]` because it compiles the playground core twice (the host library and the runner);
//! run it with `--ignored`, as CI does.

mod common;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::{TempDir, repo_root, run_ok, shared_target, undra};

/// The playground core's schema hash. A change to the core's public surface moves it (and the
/// generated bindings, which pin it too); update it together with `examples/playground/generated`.
const PLAYGROUND_HASH: u64 = 0xabdf_844b_53e0_bc10;

fn playground() -> PathBuf {
    repo_root().join("examples/playground")
}

/// Every file below `dir`, by path relative to it.
fn tree(dir: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).expect("the output directory is readable") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(
                    name,
                    std::fs::read_to_string(&path).expect("generated files are text"),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn bindgen(out: &Path, extra: &[&str]) -> String {
    let result = run_ok(
        undra()
            .arg("-C")
            .arg(playground())
            .arg("bindgen")
            .args(extra)
            .arg("--out")
            .arg(out),
    );
    String::from_utf8_lossy(&result.stdout).into_owned()
}

/// Starts `undra dev` (which builds the runner), reads the schema hash from its banner and stops
/// it. Returns the hash and leaves the runner executable in the shared target directory.
fn runner_schema_hash() -> u64 {
    let mut child = undra()
        .arg("-C")
        .arg(playground())
        .args(["dev", "--addr", "127.0.0.1:0", "--no-watch"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("undra dev starts");
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut hash = None;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(text) = line.trim().strip_prefix("schema hash") {
            hash = u64::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok();
            break;
        }
    }
    // The runner goes away with `undra dev` (it runs until its stdin closes).
    let _ = child.kill();
    let _ = child.wait();
    hash.expect("undra dev printed a schema hash before it exited")
}

#[test]
#[ignore = "builds the playground core twice; run with --ignored (CI does)"]
fn docs_read_from_the_library_are_what_the_dev_runner_prints() {
    // The library route: `undra bindgen --docs` (builds the host library, `dlopen`s it).
    // Both outputs sit side by side, one level below the same scratch directory: the Swift
    // `Package.swift` names the runtime relative to where it is written.
    let scratch = TempDir::new("docs");
    let from_library = scratch.path().join("library");
    let report = bindgen(&from_library, &["--docs"]);
    assert!(
        report.contains(&format!("schema hash {PLAYGROUND_HASH:#018x}")),
        "the library's schema hash moved off {PLAYGROUND_HASH:#018x}:\n{report}"
    );
    let library_files = tree(&from_library);
    let types = &library_files["swift/Sources/PlaygroundCore/Generated/Types.swift"];
    assert!(
        types.contains("/// A record that nests the other kinds"),
        "`--docs` produced no documentation from the library:\n{types}"
    );

    // The runner route: its banner hash, then its full schema through `--schema`.
    assert_eq!(
        runner_schema_hash(),
        PLAYGROUND_HASH,
        "the dev runner's schema hash differs from the library's"
    );
    let runner = shared_target().join("debug/undra-dev-runner");
    assert!(
        runner.is_file(),
        "`undra dev` left no runner at {}",
        runner.display()
    );
    let printed = Command::new(&runner)
        .arg("--print-schema")
        .output()
        .expect("the runner starts");
    assert!(
        printed.status.success(),
        "--print-schema failed: {}",
        printed.status
    );
    let schema_file = scratch.path().join("schema.json");
    std::fs::write(&schema_file, &printed.stdout).unwrap();
    let from_runner = scratch.path().join("runner");
    let report = bindgen(
        &from_runner,
        &[
            "--schema",
            schema_file.to_str().unwrap(),
            "--crate-name",
            "playground-core",
        ],
    );
    assert!(
        report.contains(&format!("schema hash {PLAYGROUND_HASH:#018x}")),
        "{report}"
    );

    // The same files, byte for byte.
    let runner_files = tree(&from_runner);
    assert_eq!(
        runner_files.keys().collect::<Vec<_>>(),
        library_files.keys().collect::<Vec<_>>(),
        "the library's schema and the dev runner's generate different files"
    );
    let differing: Vec<&String> = library_files
        .iter()
        .filter(|(name, text)| runner_files[*name] != **text)
        .map(|(name, _)| name)
        .collect();
    assert!(
        differing.is_empty(),
        "the library's schema and the dev runner's generate different bindings: {differing:?}"
    );

    // They are also the committed bindings, which `--check` compares without writing anything.
    let check = run_ok(
        undra()
            .arg("-C")
            .arg(playground())
            .args(["bindgen", "--docs", "--check"]),
    );
    assert!(
        String::from_utf8_lossy(&check.stdout).contains("up to date"),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );

    // Without `--docs` the same schema generates the same code minus the documentation, at the
    // same hash.
    let undocumented = scratch.path().join("none");
    let report = bindgen(&undocumented, &[]);
    assert!(
        report.contains(&format!("schema hash {PLAYGROUND_HASH:#018x}")),
        "{report}"
    );
    let bare = tree(&undocumented);
    assert_eq!(
        bare.keys().collect::<Vec<_>>(),
        library_files.keys().collect::<Vec<_>>()
    );
    let bare_types = &bare["swift/Sources/PlaygroundCore/Generated/Types.swift"];
    assert!(
        !bare_types.contains("A record that nests the other kinds"),
        "{bare_types}"
    );
    assert!(bare_types.len() < types.len());
}
