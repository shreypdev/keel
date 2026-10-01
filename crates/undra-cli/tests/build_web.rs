//! `undra build --platform web`: a wasm module that loads and reports the schema of the core.

mod common;

use common::{has_rust_target, has_tool, init_project, run_ok, serial};

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
