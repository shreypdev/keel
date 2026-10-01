//! `undra init` into a temp directory, then the real toolchain on what it wrote.

mod common;

use std::path::{Path, PathBuf};

use common::{init_project, repo_root, run_ok, shared_target};

/// The first file called `name` below `dir`.
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|n| n == name) {
            return Some(path);
        }
    }
    None
}

/// The version in the `implementation("dev.undra:<artifact>:<version>")` line of a Gradle script.
fn dependency_version(script: &str, artifact: &str) -> String {
    let prefix = format!("implementation(\"dev.undra:{artifact}:");
    let line = script
        .lines()
        .find(|l| l.trim_start().starts_with(&prefix))
        .unwrap_or_else(|| panic!("no dev.undra:{artifact} dependency in\n{script}"));
    line.trim_start()[prefix.len()..]
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn the_generated_core_builds_its_tests_pass_and_its_bindings_are_current() {
    let project = init_project("roundtrip", "ios,android,web");

    // The core is an ordinary crate: `cargo test` works on it without Undra's help.
    let test = run_ok(
        std::process::Command::new("cargo")
            .args(["test", "--manifest-path"])
            .arg(project.root.join("core/Cargo.toml"))
            .env("CARGO_TARGET_DIR", shared_target()),
    );
    assert!(
        String::from_utf8_lossy(&test.stdout).contains("test result: ok. 2 passed"),
        "{}",
        String::from_utf8_lossy(&test.stdout)
    );

    // `undra bindgen` (build the core, dlopen it, read the schema) agrees, byte for byte, with the
    // bindings `init` wrote from the embedded schema: the embedded schema has not drifted from
    // the template core.
    let check = run_ok(project.undra().args(["bindgen", "--check"]));
    let text = String::from_utf8_lossy(&check.stdout);
    assert!(text.contains("up to date"), "{text}");
}

#[test]
fn the_project_layout_is_what_the_readme_says() {
    let project = init_project("layout", "ios,android,web");
    for file in [
        "undra.toml",
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
    let settings =
        std::fs::read_to_string(project.root.join("android/settings.gradle.kts")).unwrap();
    let include = settings
        .lines()
        .find(|l| l.contains("includeBuild("))
        .expect("a composite build of the runtime");
    let rel = include.split('"').nth(1).unwrap();
    assert!(
        project
            .root
            .join("android")
            .join(rel)
            .join("settings.gradle.kts")
            .is_file(),
        "{include} does not reach {}",
        repo.display()
    );
    let swift =
        std::fs::read_to_string(project.root.join("generated/swift/Package.swift")).unwrap();
    let dep = swift
        .split(".package(path: \"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert!(
        project
            .root
            .join("generated/swift")
            .join(dep)
            .join("Package.swift")
            .is_file(),
        "{dep}"
    );
}

#[test]
fn undra_toml_round_trips_through_the_cli() {
    let project = init_project("toml", "web");
    // `undra doctor` reads undra.toml and reports the project by name and platforms.
    let out = run_ok(project.undra().args(["doctor", "--platform", "web"]));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("checking toml (web)"), "{text}");
}

/// ADR-031: what the core produces on its own is applied once per display frame, and on Android the frame is the
/// Choreographer's, which lives in `android-adapters`, not in the Android-free runtime module.
#[test]
fn the_android_shell_installs_the_choreographer_frame_pacer() {
    let project = init_project("pacer", "android");
    let app_path = find_file(&project.root.join("android/app/src/main/kotlin"), "UndraApp.kt")
        .expect("init writes the Application class");
    let app = std::fs::read_to_string(&app_path).unwrap();
    for needle in [
        "import dev.undra.android.ChoreographerFramePacer",
        "import dev.undra.runtime.MirrorOptions",
        "mirror = MirrorOptions(framePacer = ChoreographerFramePacer())",
    ] {
        assert!(app.contains(needle), "{} lacks `{needle}`:\n{app}", app_path.display());
    }

    // The Gradle module the import comes from is a dependency, at the version the runtime has.
    let gradle = std::fs::read_to_string(project.root.join("android/app/build.gradle.kts")).unwrap();
    assert_eq!(
        dependency_version(&gradle, "android-adapters"),
        dependency_version(&gradle, "runtime"),
        "the two Undra Kotlin modules must be the same version:\n{gradle}"
    );

    // Checkout mode resolves `dev.undra:android-adapters` through the composite build of the runtime, which includes the
    // module (when an Android SDK is present) and names the class the template imports.
    let runtime = repo_root().join("runtimes/kotlin/undra-runtime");
    let settings = std::fs::read_to_string(runtime.join("settings.gradle.kts")).unwrap();
    assert!(settings.contains("include(\":android-adapters\")"), "{settings}");
    let pacer = std::fs::read_to_string(
        runtime.join("android-adapters/src/main/kotlin/dev/undra/android/ChoreographerFramePacer.kt"),
    )
    .unwrap();
    assert!(pacer.contains("public class ChoreographerFramePacer"), "{pacer}");
}
