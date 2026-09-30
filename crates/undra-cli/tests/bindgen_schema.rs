//! `undra bindgen --schema`: a schema file in, the three trees out, nothing built.

mod common;

use std::path::Path;

use common::{TempDir, undra, run_err, run_ok};

fn fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stores.schema.json")
}

/// The files compared with the goldens: one of each language, plus the two package manifests the
/// CLI adds.
const GOLDEN_FILES: &[&str] = &[
    "swift/Package.swift",
    "swift/Sources/GoldenStores/Generated/Stores.swift",
    "kotlin/build.gradle.kts",
    "kotlin/.gitignore",
    "kotlin/src/main/kotlin/dev/undra/generated/golden_stores/Stores.kt",
    "ts/package.json",
    "ts/src/stores.ts",
];

fn golden_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/stores")
}

#[test]
fn a_schema_file_generates_the_three_trees() {
    let out = TempDir::new("schema-out");
    let result = run_ok(
        undra()
            .args(["bindgen", "--schema"])
            .arg(fixture())
            .arg("--out")
            .arg(out.path())
            .current_dir(out.path()),
    );
    let text = String::from_utf8_lossy(&result.stdout);
    assert!(
        text.contains("Generated bindings for golden-stores")
            && text.contains("swift")
            && text.contains("kotlin")
            && text.contains("ts"),
        "{text}"
    );

    let updating = std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1");
    for file in GOLDEN_FILES {
        let actual = std::fs::read_to_string(out.path().join(file))
            .unwrap_or_else(|_| panic!("{file} was not generated"));
        let golden = golden_dir().join(file);
        if updating {
            std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
            std::fs::write(&golden, &actual).unwrap();
            continue;
        }
        let expected = std::fs::read_to_string(&golden)
            .unwrap_or_else(|_| panic!("golden {file} is missing; run with UPDATE_GOLDEN=1"));
        assert_eq!(
            actual, expected,
            "{file} differs from its golden (UPDATE_GOLDEN=1 to refresh after reviewing)"
        );
    }
}

#[test]
fn generation_is_idempotent_and_check_agrees() {
    let out = TempDir::new("schema-idem");
    let mut generate = undra();
    generate
        .args(["bindgen", "--schema"])
        .arg(fixture())
        .arg("--out")
        .arg(out.path());
    run_ok(&mut generate);
    let second = run_ok(&mut generate);
    assert!(
        String::from_utf8_lossy(&second.stdout).contains("0 written"),
        "{}",
        String::from_utf8_lossy(&second.stdout)
    );

    let check = run_ok(
        undra()
            .args(["bindgen", "--check", "--schema"])
            .arg(fixture())
            .arg("--out")
            .arg(out.path()),
    );
    assert!(String::from_utf8_lossy(&check.stdout).contains("up to date"));

    // A hand edit makes --check fail and name the file.
    let edited = out.path().join("ts/src/stores.ts");
    let mut text = std::fs::read_to_string(&edited).unwrap();
    text.push_str("// edited\n");
    std::fs::write(&edited, text).unwrap();
    let (code, stderr) = run_err(
        undra()
            .args(["bindgen", "--check", "--schema"])
            .arg(fixture())
            .arg("--out")
            .arg(out.path()),
    );
    assert_eq!(code, 1);
    assert!(
        stderr.contains("out of date") && stderr.contains("ts/src/stores.ts differs"),
        "{stderr}"
    );
    // Regenerating repairs it.
    run_ok(&mut generate);
    run_ok(
        undra()
            .args(["bindgen", "--check", "--schema"])
            .arg(fixture())
            .arg("--out")
            .arg(out.path()),
    );
}

#[test]
fn files_of_an_earlier_run_are_removed_and_the_users_are_not() {
    let out = TempDir::new("schema-stale");
    let mut generate = undra();
    generate
        .args(["bindgen", "--schema"])
        .arg(fixture())
        .arg("--out")
        .arg(out.path());
    run_ok(&mut generate);
    std::fs::write(out.path().join("ts/src/mine.ts"), "// mine\n").unwrap();
    // Pretend the previous run also wrote a file the schema no longer generates.
    std::fs::write(out.path().join("ts/src/gone.ts"), "// old\n").unwrap();
    let manifest = out.path().join(".undra-generated");
    let mut listed = std::fs::read_to_string(&manifest).unwrap();
    listed.push_str("ts/src/gone.ts\n");
    std::fs::write(&manifest, listed).unwrap();

    let again = run_ok(&mut generate);
    assert!(
        String::from_utf8_lossy(&again.stdout).contains("1 removed"),
        "{}",
        String::from_utf8_lossy(&again.stdout)
    );
    assert!(!out.path().join("ts/src/gone.ts").exists());
    assert!(
        out.path().join("ts/src/mine.ts").exists(),
        "files the CLI did not write are never touched"
    );
}

#[test]
fn platforms_can_be_selected() {
    let out = TempDir::new("schema-platforms");
    run_ok(
        undra()
            .args(["bindgen", "--platforms", "web", "--schema"])
            .arg(fixture())
            .arg("--out")
            .arg(out.path()),
    );
    assert!(out.path().join("ts/src/index.ts").is_file());
    assert!(!out.path().join("swift").exists() && !out.path().join("kotlin").exists());
}

#[test]
fn a_broken_schema_file_is_explained() {
    let out = TempDir::new("schema-broken");
    let bad = out.path().join("bad.json");
    std::fs::write(&bad, "{\"records\": 3}").unwrap();
    let (code, stderr) = run_err(
        undra()
            .args(["bindgen", "--schema"])
            .arg(&bad)
            .arg("--out")
            .arg(out.path()),
    );
    assert_eq!(code, 1);
    assert!(
        stderr.contains("error[undra::C0002]")
            && stderr.contains("bad.json")
            && stderr.contains("expected shape"),
        "{stderr}"
    );
    let (_, stderr) = run_err(
        undra()
            .args(["bindgen", "--schema"])
            .arg(out.path().join("missing.json"))
            .arg("--out")
            .arg(out.path()),
    );
    assert!(stderr.contains("error[undra::C0010]"), "{stderr}");
}

#[test]
fn a_schema_bindgen_rejects_reports_its_diagnostics() {
    let out = TempDir::new("schema-invalid");
    // The fixture with its first record duplicated: E0050.
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture()).unwrap()).unwrap();
    let first = value["records"][0].clone();
    value["records"].as_array_mut().unwrap().push(first);
    let bad = out.path().join("dup.json");
    std::fs::write(&bad, serde_json::to_string(&value).unwrap()).unwrap();
    let (code, stderr) = run_err(
        undra()
            .args(["bindgen", "--schema"])
            .arg(&bad)
            .arg("--out")
            .arg(out.path()),
    );
    assert_eq!(code, 1);
    assert!(
        stderr.contains("error[undra::C0007]") && stderr.contains("E0050"),
        "{stderr}"
    );
}
