//! `keel adopt` on a repository that already has apps, then the result used as a project.

mod common;

use common::{TempDir, keel, repo_root, run_ok};

fn write(root: &std::path::Path, path: &str, text: &str) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

#[test]
fn adopting_adds_one_directory_that_is_a_working_project() {
    let dir = TempDir::new("adopt");
    let app = dir.path().join("MyApp");
    write(
        &app,
        "ios/MyApp.xcodeproj/project.pbxproj",
        "PRODUCT_BUNDLE_IDENTIFIER = com.acme.myapp;\n",
    );
    write(
        &app,
        "web/package.json",
        "{\"devDependencies\": {\"vite\": \"^6\"}, \"dependencies\": {\"react\": \"^19\"}}",
    );

    let out = run_ok(
        keel()
            .arg("adopt")
            .arg(&app)
            .arg("--keel-path")
            .arg(repo_root()),
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("Detected: iOS") && text.contains("web (vite"),
        "{text}"
    );
    assert!(text.contains("KEEL_ADOPT.md"), "{text}");

    // The app's files are untouched.
    assert_eq!(
        std::fs::read_to_string(app.join("ios/MyApp.xcodeproj/project.pbxproj")).unwrap(),
        "PRODUCT_BUNDLE_IDENTIFIER = com.acme.myapp;\n"
    );

    // `keel/` is a project: its bindings are current (this builds the core and loads it).
    std::fs::copy(repo_root().join("Cargo.lock"), app.join("keel/Cargo.lock")).unwrap();
    let check = run_ok(
        keel()
            .arg("-C")
            .arg(app.join("keel"))
            .args(["bindgen", "--check"]),
    );
    assert!(String::from_utf8_lossy(&check.stdout).contains("up to date"));

    // The guide has the real paths in it.
    let guide = std::fs::read_to_string(app.join("keel/KEEL_ADOPT.md")).unwrap();
    assert!(
        guide.contains(
            "-force_load $(SRCROOT)/../keel/build/ios/KeelCore.xcframework/ios-arm64/libkeel_core.a"
        ),
        "{guide}"
    );
    assert!(
        guide.contains("## Web") && guide.contains("../keel/build/web/keel_core.wasm"),
        "{guide}"
    );
}
