//! `undra build --platform web`: a wasm module that loads and reports the schema of the core.

mod common;

use common::{has_rust_target, has_tool, init_project, init_project_in, run_ok, serial};

#[test]
fn the_web_build_produces_a_loadable_wasm_core() {
    let _serial = serial();
    if !has_rust_target("wasm32-unknown-unknown") {
        eprintln!(
            "skipped: the wasm32-unknown-unknown Rust target is not installed (rustup target add wasm32-unknown-unknown)"
        );
        return;
    }
    let project = init_project("webbuild", "web");
    let out = run_ok(project.undra().args(["build", "--platform", "web"]));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let wasm = project.root.join("build/web/webbuild_core.wasm");
    assert!(wasm.is_file(), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("web wasm")
            && stdout.contains("gzip")
            && stdout.contains("budget 120 KB gzip"),
        "sizes are printed:\n{stdout}"
    );
    if has_tool("wasm-opt", "--version") {
        assert!(stderr.contains("Optimizing with wasm-opt"), "{stderr}");
    } else {
        assert!(
            stderr.contains("wasm-opt (binaryen) is not installed"),
            "a missing wasm-opt is said, not fatal:\n{stderr}"
        );
    }

    // The builder's home directory is not in what ships (ADR-052): the Undra crates of this
    // checkout and the registry crates live below it, and every panic location would name them.
    if let Some(home) = std::env::var_os("HOME").filter(|h| h.len() > 1) {
        let home = home.to_string_lossy().into_owned();
        let bytes = std::fs::read(&wasm).unwrap();
        let found = bytes
            .windows(home.len())
            .filter(|w| *w == home.as_bytes())
            .count();
        assert_eq!(
            found, 0,
            "the wasm module names the home directory {home} {found} times"
        );
    }

    // The schema hash of the module is the hash of the bindings (`undra bindgen` wrote them at
    // init): load the module under Node with stub imports and ask it.
    if !has_tool("node", "--version") {
        eprintln!("skipped the load check: node is not installed");
        return;
    }
    let ids = std::fs::read_to_string(project.root.join("generated/ts/src/ids.ts")).unwrap();
    let expected = ids
        .lines()
        .find_map(|l| l.trim().strip_prefix("schemaHash: "))
        .expect("ids.ts has the schema hash")
        .trim_end_matches(['n', ',']);
    let script = r#"
        const fs = require("fs");
        const module = new WebAssembly.Module(fs.readFileSync(process.argv[1]));
        const imports = { undra: {} };
        for (const i of WebAssembly.Module.imports(module)) imports[i.module][i.name] = () => 0;
        WebAssembly.instantiate(module, imports).then((instance) => {
          const e = instance.exports;
          if (e._initialize) e._initialize();
          console.log(e.undra_abi_version() + " 0x" + BigInt.asUintN(64, e.undra_schema_hash()).toString(16).padStart(16, "0"));
        });
    "#;
    let loaded = run_ok(
        std::process::Command::new("node")
            .args(["-e", script])
            .arg(&wasm),
    );
    let printed = String::from_utf8_lossy(&loaded.stdout).trim().to_owned();
    assert_eq!(
        printed,
        format!("1 {expected}"),
        "abi version 1 and the hash of the bindings"
    );
}

/// The web module does not depend on where the project is checked out (ADR-052): panic locations name
/// the project, the Undra checkout and the registry crates by fixed labels, not by their paths, so the
/// gzipped size of the hello world does not move with the length of the directory it was built in.
/// Two copies of one project, at paths of different length, carry the same strings, and none of the
/// paths (nor the Undra checkout's, nor the home directory's) is in either.
///
/// Not *every* byte is the same: a `TypeId` is a hash of the crate's identity, and Cargo derives that
/// from the absolute path of a path dependency that sits outside the shim's workspace (the core, and
/// the Undra checkout), which no flag changes. The constants that hold them differ in value and, as
/// LEB128, by a byte or two in width; the test bounds that (and its effect on the gzipped size)
/// instead of pretending to byte-identity.
#[test]
fn the_web_module_does_not_depend_on_where_the_project_lives() {
    let _serial = serial();
    if !has_rust_target("wasm32-unknown-unknown") {
        eprintln!("skipped: the wasm32-unknown-unknown Rust target is not installed");
        return;
    }
    let build = |tag: &str| {
        let project = init_project_in(tag, "pathindep", "web");
        run_ok(project.undra().args(["build", "--platform", "web"]));
        let wasm = project.root.join("build/web/pathindep_core.wasm");
        let bytes = std::fs::read(&wasm).expect("the build wrote the module");
        (project, bytes)
    };
    let (a, bytes_a) = build("a");
    let (b, bytes_b) = build("a-directory-with-a-much-longer-name-than-the-first-one");
    assert!(b.root.as_os_str().len() > a.root.as_os_str().len() + 30);

    // No path of this machine ships.
    let mut names = vec![
        a.root.to_string_lossy().into_owned(),
        b.root.to_string_lossy().into_owned(),
        common::repo_root().to_string_lossy().into_owned(),
    ];
    if let Some(home) = std::env::var_os("HOME").filter(|h| h.len() > 1) {
        names.push(home.to_string_lossy().into_owned());
    }
    for (module, bytes) in [("first", &bytes_a), ("second", &bytes_b)] {
        for name in &names {
            let found = bytes
                .windows(name.len())
                .filter(|w| *w == name.as_bytes())
                .count();
            assert_eq!(found, 0, "the {module} module names {name}");
        }
    }

    // Every string of a path's length is the same in both: a hash does not read as sixteen printable
    // bytes in a row, a path does.
    let strings = |bytes: &[u8]| -> Vec<String> {
        let mut found = Vec::new();
        let mut run = Vec::new();
        for &byte in bytes.iter().chain(std::iter::once(&0)) {
            if (0x20..0x7f).contains(&byte) {
                run.push(byte);
            } else {
                if run.len() >= 16 {
                    found.push(String::from_utf8_lossy(&run).into_owned());
                }
                run.clear();
            }
        }
        found
    };
    let (strings_a, strings_b) = (strings(&bytes_a), strings(&bytes_b));
    assert!(
        strings_a.iter().any(|s| s.contains("/undra/src/crates/")),
        "the Undra crates are named by their label"
    );
    assert_eq!(
        strings_a, strings_b,
        "the two copies carry different strings: something names where it was built"
    );
    // The TypeId constants: the same count, so at most a byte or two each in width.
    let (len_a, len_b) = (bytes_a.len() as i64, bytes_b.len() as i64);
    assert!(
        (len_a - len_b).abs() <= 16,
        "{len_a} against {len_b} bytes: more than the width of a few hashes"
    );
}

#[test]
fn a_second_build_reuses_the_first() {
    let _serial = serial();
    if !has_rust_target("wasm32-unknown-unknown") {
        return;
    }
    let project = init_project("webrebuild", "web");
    run_ok(project.undra().args(["build", "--platform", "web"]));
    let started = std::time::Instant::now();
    let again = run_ok(project.undra().args(["build", "--platform", "web"]));
    // (Serialized with the other tests of this file: they share the target directory.)
    let stderr = String::from_utf8_lossy(&again.stderr);
    assert!(
        !stderr.contains("Compiling webrebuild-core")
            && !stderr.contains("Compiling undra-core-shim"),
        "an unchanged project recompiles nothing of its own:\n{stderr}"
    );
    assert!(started.elapsed().as_secs() < 30);
}

#[test]
fn the_host_build_is_named_after_the_namespace() {
    let _serial = serial();
    let project = init_project("hostbuild", "web");
    let out = run_ok(project.undra().args(["build", "--platform", "host"]));
    let stdout = String::from_utf8_lossy(&out.stdout);
    // ADR-044: the core's namespace, by default its package name in snake case.
    let name = if cfg!(target_os = "macos") {
        "libhostbuild_core.dylib"
    } else if cfg!(windows) {
        "hostbuild_core.dll"
    } else {
        "libhostbuild_core.so"
    };
    assert!(
        project.root.join("build/host").join(name).is_file(),
        "{stdout}"
    );
}
