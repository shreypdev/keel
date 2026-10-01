//! `undra build --platform host`: the library the JVM and `undra bindgen` load.

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
    run_ok(project.undra().args(["build", "--platform", "host"]));
    // Named after the core's namespace (ADR-044): `hostbuild_core`.
    let library = project.root.join("build/host/libhostbuild_core.dylib");
    assert!(library.is_file(), "no library at {}", library.display());
    assert_eq!(
        install_name(&library),
        "@rpath/libhostbuild_core.dylib",
        "an absolute install name would put the build machine's target/ path into every app that embeds the library"
    );
}

/// The version Cargo locked for `package` in a `Cargo.lock`.
fn locked_version(lock: &std::path::Path, package: &str) -> Option<String> {
    let text = std::fs::read_to_string(lock).ok()?;
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line == format!("name = \"{package}\"") {
            return lines
                .next()
                .and_then(|l| l.strip_prefix("version = "))
                .map(|v| v.trim_matches('"').to_owned());
        }
    }
    None
}

#[test]
fn a_core_inside_a_workspace_builds_into_the_workspace_target() {
    let _serial = serial();
    let project = init_project("wsbuild", "web");
    let outer = project.dir.path().to_path_buf();

    // Turn `<tmp>/wsbuild` (a project that is its own workspace) into a member of the larger
    // workspace at `<tmp>`: the Undra repository's `examples/playground` is laid out this way. The
    // workspace's Cargo configuration names its target directory (and, unlike `CARGO_TARGET_DIR`,
    // is something the CLI can only learn from Cargo).
    let workspace_target = common::shared_target().join("workspace-target");
    std::fs::write(
        outer.join("Cargo.toml"),
        "[workspace]\nmembers = [\"wsbuild/core\"]\nresolver = \"3\"\n",
    )
    .unwrap();
    std::fs::remove_file(project.root.join("Cargo.toml")).unwrap();
    std::fs::rename(project.root.join("Cargo.lock"), outer.join("Cargo.lock")).unwrap();
    std::fs::create_dir_all(outer.join(".cargo")).unwrap();
    std::fs::write(
        outer.join(".cargo/config.toml"),
        format!(
            "[build]\ntarget-dir = {:?}\n",
            workspace_target.display().to_string()
        ),
    )
    .unwrap();

    run_ok(
        project
            .undra()
            .args(["build", "--platform", "host"])
            .env_remove("CARGO_TARGET_DIR")
            .current_dir(&outer),
    );

    assert!(
        !project.root.join("target").exists(),
        "a second target directory was created next to the core"
    );
    let shims: Vec<_> = std::fs::read_dir(workspace_target.join("undra"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path().join("shim"))
        .filter(|shim| {
            std::fs::read_to_string(shim.join("Cargo.toml"))
                .is_ok_and(|text| text.contains(&project.root.join("core").display().to_string()))
        })
        .collect();
    assert_eq!(
        shims.len(),
        1,
        "the shim of this project is generated below the workspace's target directory"
    );
    assert!(project.root.join("build/host").is_dir());

    // The shim resolved the versions the workspace was tested with (its lock file seeded the
    // shim's), so a dependency is compiled once for the workspace and the CLI.
    for package in ["serde_json", "parking_lot"] {
        assert_eq!(
            locked_version(&shims[0].join("Cargo.lock"), package),
            locked_version(&outer.join("Cargo.lock"), package),
            "{package}"
        );
    }
}
