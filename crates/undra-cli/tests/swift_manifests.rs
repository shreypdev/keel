//! The two manifests of the Swift runtime stay in step (ADR-063).
//!
//! `runtimes/swift/UndraRuntime/Package.swift` is the package the examples and the Swift tests use; the
//! `Package.swift` at the root of the repository is what an app adds (`.package(url:
//! "https://github.com/shreypdev/undra", from: ...)`). The root one exposes the same library products and the
//! same targets, by `path:` into the first one's sources, without the test targets. These tests read both
//! files (no Swift toolchain needed) and, where `swift` is installed, ask SwiftPM to load the root one.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::repo_root;

/// The library products: name to targets.
type Products = BTreeMap<String, Vec<String>>;

/// One target of a manifest.
#[derive(Debug, PartialEq, Eq)]
struct Target {
    dependencies: Vec<String>,
    path: String,
    public_headers: Option<String>,
    test: bool,
}

/// The call `head(` at `from` with its arguments, up to the matching parenthesis.
fn call_at(text: &str, from: usize) -> &str {
    let mut depth = 0;
    for (i, c) in text[from..].char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return &text[from..=from + i];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced call at {from}: {}", &text[from..])
}

/// The string value of `key: "value"` in `call`.
fn string_arg(call: &str, key: &str) -> Option<String> {
    let at = call.find(&format!("{key}: \""))? + key.len() + 3;
    Some(call[at..at + call[at..].find('"')?].to_owned())
}

/// The strings of `key: ["a", "b"]` in `call`.
fn list_arg(call: &str, key: &str) -> Vec<String> {
    let Some(at) = call.find(&format!("{key}: [")) else {
        return Vec::new();
    };
    let list = call_at(call, at + key.len() + 2);
    list.split('"')
        .skip(1)
        .step_by(2)
        .map(ToOwned::to_owned)
        .collect()
}

/// Every occurrence of `head` in `text` (`.library(`, `.target(`).
fn calls<'a>(text: &'a str, head: &str) -> Vec<&'a str> {
    text.match_indices(head)
        .map(|(at, _)| call_at(text, at + head.len() - 1))
        .collect()
}

fn read(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    // Comments say things like `.package(url: ...)`: only the code counts.
    text.lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn products(text: &str) -> Products {
    calls(text, ".library(")
        .into_iter()
        .map(|c| (string_arg(c, "name").unwrap(), list_arg(c, "targets")))
        .collect()
}

fn targets(text: &str) -> BTreeMap<String, Target> {
    let mut out = BTreeMap::new();
    for (head, test) in [(".target(", false), (".testTarget(", true)] {
        for call in calls(text, head) {
            out.insert(
                string_arg(call, "name").unwrap(),
                Target {
                    dependencies: list_arg(call, "dependencies"),
                    path: string_arg(call, "path").unwrap_or_default(),
                    public_headers: string_arg(call, "publicHeadersPath"),
                    test,
                },
            );
        }
    }
    out
}

/// The text of `key: [...]` (platforms, language modes), without spaces.
fn raw_list(text: &str, key: &str) -> String {
    let at = text
        .find(&format!("{key}: ["))
        .unwrap_or_else(|| panic!("no {key}"));
    call_at(text, at + key.len() + 2)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

const IN_REPO: &str = "runtimes/swift/UndraRuntime";

#[test]
fn the_root_manifest_exposes_the_runtimes_products_and_targets() {
    let root = read(&repo_root().join("Package.swift"));
    let in_repo = read(&repo_root().join(IN_REPO).join("Package.swift"));

    assert_eq!(
        products(&root),
        products(&in_repo),
        "the library products differ"
    );
    assert!(products(&root).contains_key("UndraRuntime"));

    let root_targets = targets(&root);
    let in_repo_targets = targets(&in_repo);
    let shipped: BTreeMap<&String, &Target> =
        in_repo_targets.iter().filter(|(_, t)| !t.test).collect();
    assert_eq!(
        root_targets.keys().collect::<Vec<_>>(),
        shipped.keys().copied().collect::<Vec<_>>(),
        "the root manifest has every target of the runtime but its tests, and nothing else"
    );
    for (name, target) in &root_targets {
        let theirs = shipped[name];
        assert!(!target.test, "{name}: the root manifest builds no tests");
        assert_eq!(
            target.dependencies, theirs.dependencies,
            "{name}: dependencies"
        );
        assert_eq!(
            target.public_headers, theirs.public_headers,
            "{name}: public headers"
        );
        assert_eq!(
            target.path,
            format!("{IN_REPO}/{}", theirs.path),
            "{name}: the root target builds the runtime's own sources"
        );
        assert!(
            repo_root().join(&target.path).is_dir(),
            "{name}: {} is not a directory",
            target.path
        );
    }

    assert_eq!(
        raw_list(&root, "platforms"),
        raw_list(&in_repo, "platforms")
    );
    assert_eq!(
        raw_list(&root, "swiftLanguageModes"),
        raw_list(&in_repo, "swiftLanguageModes")
    );
    let tools = |text: &str| text.lines().next().unwrap_or("").to_owned();
    assert_eq!(
        tools(&std::fs::read_to_string(repo_root().join("Package.swift")).unwrap()),
        tools(&std::fs::read_to_string(repo_root().join(IN_REPO).join("Package.swift")).unwrap()),
        "swift-tools-version"
    );
}

#[test]
fn the_root_package_is_named_by_the_identity_swiftpm_gives_the_repository() {
    // A dependency names the package by the last component of its URL (`.product(name: "UndraRuntime", package:
    // "undra")`, which `undra bindgen` writes): the manifest's own name says the same, so Xcode shows one name.
    let root = read(&repo_root().join("Package.swift"));
    let package = calls(&root, "Package(")[0];
    assert_eq!(string_arg(package, "name").as_deref(), Some("undra"));
}

#[test]
fn swiftpm_loads_the_root_manifest_where_swift_is_installed() {
    let Ok(out) = std::process::Command::new("swift")
        .args(["package", "--package-path"])
        .arg(repo_root())
        .args(["dump-package"])
        .output()
    else {
        eprintln!("skipped: swift is not installed");
        return;
    };
    assert!(
        out.status.success(),
        "swift package dump-package refused the root manifest:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = json["products"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert_eq!(names, ["UndraRuntime", "UndraTestKit"], "{json}");
}
