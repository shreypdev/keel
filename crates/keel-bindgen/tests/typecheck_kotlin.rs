//! Compiles the generated Kotlin of every golden case against the real wire
//! layer of the Kotlin runtime and the hand-written base API in
//! `tests/fixtures/kotlin-base/KeelBase.kt`, using `scripts/kotlinc.sh`.
//! Skipped, with a message on stderr, when no JVM or Kotlin compiler is
//! available; `KEEL_REQUIRE_TOOLCHAINS=1` turns the skip into a failure and
//! `KEEL_SKIP_KOTLIN=1` skips this (slow) test on purpose.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use common::{manifest_dir, on_path, repo_root, scratch, skip};
use keel_bindgen::GeneratedFile;

/// The pieces needed to compile Kotlin: the shim and the coroutines jar.
struct Toolchain {
    kotlinc: PathBuf,
    coroutines: PathBuf,
}

fn toolchain() -> Option<Toolchain> {
    if std::env::var("KEEL_SKIP_KOTLIN").is_ok_and(|v| v == "1") {
        return None;
    }
    if !on_path("java") {
        return None;
    }
    let gradle_lib =
        PathBuf::from(std::env::var("GRADLE_HOME").unwrap_or_else(|_| "/opt/gradle".to_owned()))
            .join("lib");
    if !on_path("kotlinc")
        && !gradle_lib
            .join("kotlin-compiler-embeddable-2.0.21.jar")
            .exists()
    {
        return None;
    }
    let coroutines = std::env::var("KEEL_KOTLINX_COROUTINES")
        .map(PathBuf::from)
        .unwrap_or_else(|_| gradle_lib.join("kotlinx-coroutines-core-jvm-1.6.4.jar"));
    if !coroutines.exists() {
        return None;
    }
    Some(Toolchain {
        kotlinc: repo_root().join("scripts/kotlinc.sh"),
        coroutines,
    })
}

fn kotlin_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            kotlin_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "kt") {
            out.push(path);
        }
    }
}

/// Compiles the real wire layer and the hand-written base once.
fn base_classes(tc: &Toolchain) -> &'static Path {
    static BASE: OnceLock<PathBuf> = OnceLock::new();
    BASE.get_or_init(|| {
        let out = scratch("kotlin-base-classes");
        let wire = repo_root().join("runtimes/kotlin/keel-runtime/runtime/src/main/kotlin");
        let output = Command::new(&tc.kotlinc)
            .arg("-cp")
            .arg(&tc.coroutines)
            .arg("-d")
            .arg(&out)
            .args(["-jvm-target", "11"])
            .arg(&wire)
            .arg(manifest_dir().join("tests/fixtures/kotlin-base/KeelBase.kt"))
            .output()
            .expect("kotlinc.sh runs");
        assert!(
            output.status.success(),
            "compiling the Kotlin wire layer and base fixture failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        out
    })
}

/// The Kotlin standard library, needed to run compiled code.
fn stdlib() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("KEEL_KOTLIN_STDLIB") {
        return Some(PathBuf::from(path));
    }
    let gradle_lib =
        PathBuf::from(std::env::var("GRADLE_HOME").unwrap_or_else(|_| "/opt/gradle".to_owned()))
            .join("lib");
    let jar = gradle_lib.join("kotlin-stdlib-2.0.21.jar");
    jar.exists().then_some(jar)
}

/// The `main` classes of `tests/fixtures/kotlin-run`: package and class.
const MAINS: &[(&str, &str)] = &[
    ("full", "FullTestKt"),
    ("errors", "ErrorsTestKt"),
    ("enums", "EnumsTestKt"),
    ("records", "RecordsTestKt"),
];

/// Generates every golden case, each in its own package, compiles all of them
/// together with the execution tests of `tests/fixtures/kotlin-run` (warnings
/// are errors) and runs those against the real wire layer and a fake core.
#[test]
fn every_case_compiles_and_the_generated_code_behaves() {
    let Some(tc) = toolchain() else {
        skip("no Kotlin toolchain (java plus kotlinc or a Gradle distribution) found");
        return;
    };
    let base = base_classes(&tc);
    let src = scratch("kotlin-generated-src");
    for case in common::CASES {
        let schema = common::case(case);
        let generator = common::generator_for(case, &schema);
        let files = generator.kotlin(&schema).unwrap();
        GeneratedFile::write_all(&files, &src.join(case)).unwrap();
    }
    let mut sources = Vec::new();
    kotlin_files(&src, &mut sources);
    kotlin_files(
        &manifest_dir().join("tests/fixtures/kotlin-run"),
        &mut sources,
    );
    sources.sort();
    let classes = scratch("kotlin-generated-classes");
    let classpath = format!("{}:{}", base.display(), tc.coroutines.display());
    let output = Command::new(&tc.kotlinc)
        .arg("-cp")
        .arg(&classpath)
        .arg("-d")
        .arg(&classes)
        .args(["-jvm-target", "11", "-Werror"])
        .args(&sources)
        .output()
        .expect("kotlinc.sh runs");
    assert!(
        output.status.success(),
        "the generated Kotlin does not compile:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let Some(stdlib) = stdlib() else {
        skip("no Kotlin standard library jar found; the generated Kotlin was compiled but not run");
        return;
    };
    let runtime_cp = format!("{}:{}:{}", classes.display(), classpath, stdlib.display());
    for (case, main) in MAINS {
        let output = Command::new("java")
            .arg("-cp")
            .arg(&runtime_cp)
            .arg(format!("golden.{case}.{main}"))
            .output()
            .expect("java runs");
        assert!(
            output.status.success(),
            "the generated Kotlin of `{case}` fails its execution test:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
