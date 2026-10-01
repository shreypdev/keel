//! Compiles the generated Swift of every golden case against the real Swift runtime
//! (`runtimes/swift/UndraRuntime`) in the Swift 6 language mode, which is strict concurrency,
//! and runs the execution checks of `tests/fixtures/swift-run` against it.
//!
//! This is also what proves the standard library contract of ADR-024 (amended): the stdlib case
//! refers to `HttpRequest`, `HttpError`, `NetKind`, ... and declares none of them, so it only
//! builds if the runtime exports every one of them as public API with the conformances generated
//! code uses (`UndraRecord`, `UndraError`, `Codable`, public initializers).
//!
//! Every case becomes one target of a single scratch SwiftPM package below the target
//! directory, so that one `swift build` type-checks all of them and nothing is written
//! into the repository. Each case also brings the C module bindgen generates for its core,
//! `<Namespace>CoreFFI` (the declaration of `<namespace>_undra_api`, ADR-044), which its Swift
//! target depends on; an execution check, which is linked, gets a C target that defines that
//! function as returning NULL (the checks never load a core in process). A compiler error fails the test; warnings do not (the generator
//! still escapes a few keyword argument labels that Swift accepts bare). Skipped, with a
//! message on stderr, when `swift` is not on the path or the host is not macOS (the
//! runtime builds against the Apple SDKs); `UNDRA_REQUIRE_TOOLCHAINS=1` turns the skip
//! into a failure and `UNDRA_SKIP_SWIFT=1` skips this test on purpose.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{manifest_dir, on_path, repo_root, scratch, skip};
use undra_bindgen::GeneratedFile;

/// The SwiftPM target a case is compiled as: `records` is `GoldenRecords`.
fn target_name(case: &str) -> String {
    let mut chars = case.chars();
    let first = chars.next().map(|c| c.to_ascii_uppercase());
    format!("Golden{}{}", first.unwrap_or_default(), chars.as_str())
}

/// The execution checks: a case and the `main.swift` (in `tests/fixtures/swift-run`) that is built
/// against its generated module as the executable `<target>Check`.
const CHECKS: &[(&str, &str)] = &[("recursive", "recursive.swift")];

/// The executable target of a check.
fn check_name(case: &str) -> String {
    format!("{}Check", target_name(case))
}

/// The C module of a case's core (`GoldenRecordsCoreFFI`, `PlaygroundCoreFFI`), as generated.
fn ffi_module(case: &str) -> String {
    common::generator_for(case, &common::case(case))
        .core_names()
        .ffi_module()
}

/// The C target that stands in for a check's core at link time.
fn stub_name(case: &str) -> String {
    format!("{}CoreStub", check_name(case))
}

/// The source of `stub_name(case)`: the core's entry point, returning no table.
fn stub_source(case: &str) -> String {
    let symbol = common::generator_for(case, &common::case(case))
        .core_names()
        .api_symbol();
    format!(
        "/* A stand-in for the core's entry point: the check never loads the core in process. */
         const void *{symbol}(void);
         const void *{symbol}(void) {{ return 0; }}
"
    )
}

/// Why the Swift toolchain cannot be used, or `None` when it can.
fn unavailable() -> Option<&'static str> {
    if std::env::var("UNDRA_SKIP_SWIFT").is_ok_and(|v| v == "1") {
        Some("UNDRA_SKIP_SWIFT=1")
    } else if !cfg!(target_os = "macos") {
        Some("the Swift runtime builds on macOS only")
    } else if !on_path("swift") {
        Some("no `swift` found on the path")
    } else {
        None
    }
}

/// The manifest of the scratch package: per case, the C module of its core and its Swift target;
/// per check, the stand-in for the core and the executable; all on the real runtime.
fn manifest(runtime: &Path, cases: &[&str], checks: &[(&str, &str)]) -> String {
    let runtime_product = ".product(name: \"UndraRuntime\", package: \"UndraRuntime\")";
    let mut targets: String = cases
        .iter()
        .map(|case| {
            let ffi = ffi_module(case);
            format!(
                "        .target(name: \"{ffi}\", path: \"Sources/{ffi}\", publicHeadersPath: \"include\"),\n        \
                 .target(name: \"{}\", dependencies: [\"{ffi}\", {runtime_product}]),\n",
                target_name(case)
            )
        })
        .collect();
    for (case, _) in checks {
        targets.push_str(&format!(
            "        .target(name: \"{stub}\", path: \"Sources/{stub}\"),\n        \
             .executableTarget(name: \"{}\", dependencies: [\"{}\", {runtime_product}, \"{stub}\"]),\n",
            check_name(case),
            target_name(case),
            stub = stub_name(case)
        ));
    }
    format!(
        "// swift-tools-version: 6.0\n\
         import PackageDescription\n\n\
         let package = Package(\n    \
             name: \"GoldenSwift\",\n    \
             platforms: [.macOS(.v14)],\n    \
             dependencies: [.package(path: \"{}\")],\n    \
             targets: [\n{targets}    ],\n    \
             swiftLanguageModes: [.v6]\n\
         )\n",
        runtime.display()
    )
}

/// Writes the generated Swift of `cases` as the targets of one package in `root`.
fn lay_out(root: &Path, runtime: &Path, cases: &[&str]) {
    for case in cases {
        let schema = common::case(case);
        let generator = common::generator_for(case, &schema);
        let files = generator.swift(&schema).unwrap();
        // The generated Swift never names its own module, so a case compiles under any target
        // name; the golden path's module (`PlaygroundCore`, `GoldenRecords`, ...) is replaced by
        // the case's own. The C module of the core keeps its generated name and layout: the
        // Swift imports it by that name.
        let ffi = format!("Sources/{}/", ffi_module(case));
        let renamed: Vec<GeneratedFile> = files
            .into_iter()
            .map(|f| {
                if f.path.starts_with(&ffi) {
                    return f;
                }
                let file = f.path.rsplit('/').next().unwrap_or(&f.path).to_owned();
                GeneratedFile {
                    path: format!("Sources/{}/Generated/{file}", target_name(case)),
                    contents: f.contents,
                }
            })
            .collect();
        assert!(
            renamed.iter().any(|f| f.path.starts_with(&ffi)),
            "`{case}` generates no C module {ffi}"
        );
        GeneratedFile::write_all(&renamed, root).unwrap();
    }
    for (case, fixture) in CHECKS {
        let stub = root.join("Sources").join(stub_name(case));
        fs::create_dir_all(stub.join("include")).unwrap();
        fs::write(stub.join("core_stub.c"), stub_source(case)).unwrap();
        let dir = root.join("Sources").join(check_name(case));
        fs::create_dir_all(&dir).unwrap();
        fs::copy(
            manifest_dir()
                .join("tests/fixtures/swift-run")
                .join(fixture),
            dir.join("main.swift"),
        )
        .unwrap();
    }
    fs::write(root.join("Package.swift"), manifest(runtime, cases, CHECKS)).unwrap();
}

/// The compiler diagnostics that are errors, one per line, for a readable failure.
fn errors(output: &str) -> String {
    output
        .lines()
        .filter(|l| l.contains(": error:"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether the build left the compiled module `target` (SwiftPM 6 keeps them in `Modules/`).
fn has_module(root: &Path, target: &str) -> bool {
    let debug = root.join(".build/debug");
    [debug.join("Modules"), debug]
        .iter()
        .any(|dir| dir.join(format!("{target}.swiftmodule")).exists())
}

fn build(root: &Path) -> Result<String, String> {
    let output = Command::new("swift")
        .arg("build")
        .arg("--package-path")
        .arg(root)
        .arg("--scratch-path")
        .arg(root.join(".build"))
        // The manifest is ours; a nested sandbox (CI inside a container, an agent) would only
        // make the manifest evaluation fail.
        .arg("--disable-sandbox")
        .output()
        .expect("swift runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

#[test]
fn every_case_compiles_and_the_generated_swift_behaves() {
    if let Some(why) = unavailable() {
        skip(&format!("no Swift toolchain: {why}"));
        return;
    }
    let runtime: PathBuf = repo_root()
        .join("runtimes/swift/UndraRuntime")
        .canonicalize()
        .expect("the Swift runtime package exists");
    let root = scratch("swift-generated");
    lay_out(&root, &runtime, common::CASES);
    match build(&root) {
        Ok(output) => {
            assert!(
                output.contains("Build complete"),
                "swift build did not report a complete build:\n{output}"
            );
            // Every case was compiled into a module of its own.
            let missing: Vec<String> = common::CASES
                .iter()
                .map(|case| target_name(case))
                .filter(|target| !has_module(&root, target))
                .collect();
            assert!(missing.is_empty(), "no compiled module for {missing:?}");
        }
        Err(output) => panic!(
            "the generated Swift does not compile ({} golden cases as one package):\n{}\n\nfull output:\n{output}",
            common::CASES.len(),
            errors(&output)
        ),
    }
    for (case, _) in CHECKS {
        let output = Command::new(root.join(".build/debug").join(check_name(case)))
            .output()
            .expect("the check executable runs");
        assert!(
            output.status.success(),
            "the generated Swift of `{case}` fails its execution test:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn the_scratch_package_names_one_target_per_case_and_one_executable_per_check() {
    // Needs no Swift toolchain: the layout is plain text.
    let manifest = manifest(Path::new("/runtime"), common::CASES, CHECKS);
    for case in common::CASES {
        let ffi = ffi_module(case);
        assert!(manifest.contains(&format!(
            ".target(name: \"{}\", dependencies: [\"{ffi}\", ",
            target_name(case)
        )));
        assert!(manifest.contains(&format!(
            ".target(name: \"{ffi}\", path: \"Sources/{ffi}\", publicHeadersPath: \"include\")"
        )));
    }
    assert!(manifest.contains("\"PlaygroundCoreFFI\""), "the full case keeps its core's module name");
    for (case, fixture) in CHECKS {
        assert!(manifest.contains(&format!(
            ".executableTarget(name: \"{}\", dependencies: [\"{}\"",
            check_name(case),
            target_name(case)
        )));
        assert!(manifest.contains(&format!("\"{}\"]", stub_name(case))));
        assert!(stub_source(case).contains("_undra_api(void) { return 0; }"));
        assert!(
            manifest_dir()
                .join("tests/fixtures/swift-run")
                .join(fixture)
                .exists(),
            "{fixture} is missing"
        );
        assert!(common::CASES.contains(case), "{case} is not a golden case");
    }
    assert!(manifest.contains("swiftLanguageModes: [.v6]"));
    assert!(manifest.contains(".package(path: \"/runtime\")"));
}
