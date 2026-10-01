//! Compiles the generated Swift of every golden case against the real Swift runtime
//! (`runtimes/swift/UndraRuntime`), in Swift 6 language mode as the generated Package.swift asks.
//!
//! This is what proves the standard library contract of ADR-024 (amended): the stdlib case refers
//! to `HttpRequest`, `HttpError`, `NetKind`, ... and declares none of them, so it only builds if
//! the runtime exports every one of them as public API with the conformances generated code uses
//! (`UndraRecord`, `UndraError`, `Codable`, public initializers).
//!
//! The runtime imports Apple frameworks (Security, Network), so this runs on macOS only; elsewhere
//! it skips, with a message on stderr. `UNDRA_REQUIRE_TOOLCHAINS=1` (CI sets it) turns a missing
//! toolchain into a failure, and `UNDRA_SKIP_SWIFT=1` skips this (slow) test on purpose.

mod common;

use std::fs;
use std::process::Command;

use common::{on_path, repo_root, scratch, skip};
use undra_bindgen::GeneratedFile;

/// The modules `generator.swift` wrote below `Sources/`, in a stable order.
fn modules(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir.join("Sources"))
        .expect("the generated package has Sources/")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn every_case_compiles_against_the_swift_runtime() {
    if std::env::var("UNDRA_SKIP_SWIFT").is_ok_and(|v| v == "1") {
        eprintln!("skipping: UNDRA_SKIP_SWIFT=1");
        return;
    }
    if !cfg!(target_os = "macos") {
        // Not a missing toolchain: the Swift runtime does not build off Apple platforms.
        eprintln!("skipping: the Swift runtime builds only on macOS");
        return;
    }
    if !on_path("swift") {
        skip("no Swift toolchain (`swift --version` does not run)");
        return;
    }

    let package = scratch("swift-generated");
    for case in common::CASES {
        let schema = common::case(case);
        let generator = common::generator_for(case, &schema);
        let files = generator.swift(&schema).unwrap();
        GeneratedFile::write_all(&files, &package).unwrap();
    }
    let modules = modules(&package);
    assert_eq!(
        modules.len(),
        common::CASES.len(),
        "one module per case: {modules:?}"
    );

    let runtime = repo_root()
        .join("runtimes/swift/UndraRuntime")
        .canonicalize()
        .expect("the Swift runtime is in the repository");
    let products: Vec<String> = modules
        .iter()
        .map(|m| format!(".library(name: \"{m}\", targets: [\"{m}\"])"))
        .collect();
    let targets: Vec<String> = modules
        .iter()
        .map(|m| {
            format!(
                ".target(name: \"{m}\", dependencies: [.product(name: \"UndraRuntime\", package: \"UndraRuntime\")], path: \"Sources/{m}\")"
            )
        })
        .collect();
    let manifest = format!(
        "// swift-tools-version: 6.0\n\
         import PackageDescription\n\
         let package = Package(\n\
         \x20   name: \"GoldenSwift\",\n\
         \x20   platforms: [.iOS(.v17), .macOS(.v14)],\n\
         \x20   products: [{}],\n\
         \x20   dependencies: [.package(path: \"{}\")],\n\
         \x20   targets: [{}],\n\
         \x20   swiftLanguageModes: [.v6]\n\
         )\n",
        products.join(", "),
        runtime.display(),
        targets.join(", ")
    );
    fs::write(package.join("Package.swift"), manifest).unwrap();

    let output = Command::new("swift")
        .arg("build")
        .arg("--package-path")
        .arg(&package)
        .output()
        .expect("swift runs");
    assert!(
        output.status.success(),
        "the generated Swift does not compile against the runtime:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
