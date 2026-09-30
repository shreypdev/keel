//! `keel build --platform host`: the library the JVM and `keel bindgen` load.

mod common;

use common::{has_tool, init_project, run_ok, serial};

/// The install name of a Mach-O dylib, from `otool -D` (its second line).
fn install_name(dylib: &std::path::Path) -> String {
    let out = run_ok(std::process::Command::new("otool").arg("-D").arg(dylib));
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .nth(1)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

#[test]
fn the_host_library_does_not_carry_the_path_of_the_machine_that_built_it() {
    let _serial = serial();
    if !cfg!(target_os = "macos") || !has_tool("otool", "--version") {
        eprintln!("skipped: install names are a macOS (Mach-O) matter");
        return;
    }
    let project = init_project("hostbuild", "web");
    run_ok(project.keel().args(["build", "--platform", "host"]));
    let library = project.root.join("build/host/libkeel_core.dylib");
    assert!(library.is_file(), "no library at {}", library.display());
    assert_eq!(
        install_name(&library),
        "@rpath/libkeel_core.dylib",
        "an absolute install name would put the build machine's target/ path into every app that embeds the library"
    );
}
