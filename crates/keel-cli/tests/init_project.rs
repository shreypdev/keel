//! `keel init` into a temp directory, then the real toolchain on what it wrote.

mod common;

use common::{init_project, repo_root, run_ok, shared_target};

#[test]
fn the_generated_core_builds_its_tests_pass_and_its_bindings_are_current() {
    let project = init_project("roundtrip", "ios,android,web");

    // The core is an ordinary crate: `cargo test` works on it without Keel's help.
    let test = run_ok(
        std::process::Command::new("cargo")
            .args(["test", "--manifest-path"])
            .arg(project.root.join("core/Cargo.toml"))
            .env("CARGO_TARGET_DIR", shared_target()),
    );
    assert!(String::from_utf8_lossy(&test.stdout).contains("test result: ok. 2 passed"), "{}", String::from_utf8_lossy(&test.stdout));

    // `keel bindgen` (build the core, dlopen it, read the schema) agrees, byte for byte, with the
    // bindings `init` wrote from the embedded schema: the embedded schema has not drifted from
    // the template core.
    let check = run_ok(project.keel().args(["bindgen", "--check"]));
    let text = String::from_utf8_lossy(&check.stdout);
    assert!(text.contains("up to date"), "{text}");
}

#[test]
fn the_project_layout_is_what_the_readme_says() {
    let project = init_project("layout", "ios,android,web");
    for file in [
        "keel.toml",
        "README.md",
        "core/src/lib.rs",
        "generated/swift/Package.swift",
        "generated/kotlin/build.gradle.kts",
        "generated/ts/package.json",
        "ios/Layout.xcodeproj/project.pbxproj",
        "android/app/build.gradle.kts",
        "web/vite.config.ts",
    ] {
        assert!(project.root.join(file).is_file(), "missing {file}");
    }
    // Checkout mode: the shells point at the checkout's runtimes by relative path.
    let repo = repo_root();
    let settings = std::fs::read_to_string(project.root.join("android/settings.gradle.kts")).unwrap();
    let include = settings.lines().find(|l| l.contains("includeBuild(")).expect("a composite build of the runtime");
    let rel = include.split('"').nth(1).unwrap();
    assert!(project.root.join("android").join(rel).join("settings.gradle.kts").is_file(), "{include} does not reach {}", repo.display());
    let swift = std::fs::read_to_string(project.root.join("generated/swift/Package.swift")).unwrap();
    let dep = swift.split(".package(path: \"").nth(1).unwrap().split('"').next().unwrap();
    assert!(project.root.join("generated/swift").join(dep).join("Package.swift").is_file(), "{dep}");
}

#[test]
fn keel_toml_round_trips_through_the_cli() {
    let project = init_project("toml", "web");
    // `keel doctor` reads keel.toml and reports the project by name and platforms.
    let out = run_ok(project.keel().args(["doctor", "--platform", "web"]));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("checking toml (web)"), "{text}");
}
