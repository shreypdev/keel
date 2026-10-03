//! Regression guard for ADR-029 and ADR-044: a cdylib `undra build` produces keeps the app core's
//! schema registrations when it is *loaded* (not linked against), and exports exactly the core's
//! entry, `<namespace>_undra_api`, and `JNI_OnLoad` -- none of C ABI version 1's global `undra_*`
//! functions and no `Java_*` natives.
//!
//! The platform runtimes load the core with no link-time reference to it: `undra bindgen` dlopens it
//! for the schema, and the generated Kotlin `UndraCoreNative` `System.loadLibrary`s it and has its
//! `JNI_OnLoad` register the natives. An incremental build on macOS dead-strips the app core's
//! `inventory::submit!` statics (the schema, SPEC 2.4) because they live in a dependency rlib the
//! linker prunes. This test builds the playground core through the real `undra` binary and asserts,
//! on the loaded library, that nothing was stripped and nothing else is exported.
//!
//! `#[ignore]` because it runs a full `undra build` (compiles the core and the shim cdylib); run it
//! with `--ignored`, as CI does. It deliberately does not pin `CARGO_INCREMENTAL`: `undra build`
//! forces it off itself, so leaving the environment's default (incremental on, in a dev shell and
//! in CI) is what makes this a real regression test of the fix.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The hash of the canonical empty schema: what `undra_schema_hash` reports when the registrations
/// were dead-stripped (`undra_meta::Schema::new(..).hash()`).
const EMPTY_SCHEMA_HASH: u64 = 0x9875_4cbe_a76a_32b2;

fn playground() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/playground")
}

/// The playground's namespace: the default, its package name in snake case.
const NAMESPACE: &str = "playground_core";

fn built_core(project: &Path) -> Option<PathBuf> {
    let host = project.join("build/host");
    for name in [
        "libplayground_core.dylib",
        "libplayground_core.so",
        "playground_core.dll",
    ] {
        let candidate = host.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Every file below `dir`, as paths relative to it.
fn files_below(dir: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.push(path.strip_prefix(root).expect("below root").to_owned());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// ADR-061: a build system that has built the core already (Bazel) hands `undra bindgen` the library, and gets exactly the
/// bindings `undra bindgen` writes after building one itself: the playground's committed tree.
#[test]
#[ignore = "runs a full `undra build`; run with --ignored (CI does)"]
fn bindgen_reads_the_schema_from_a_library_that_was_built_already() {
    let project = playground();
    let status = Command::new(env!("CARGO_BIN_EXE_undra"))
        .args(["build", "-C"])
        .arg(&project)
        .args(["--platform", "host"])
        .status()
        .expect("spawn `undra build`");
    assert!(status.success(), "`undra build --platform host` failed");
    let lib =
        built_core(&project).expect("undra build did not leave build/host/libplayground_core.*");

    // The lint fragments name the directory the tree is written to, as the committed tree's say `generated`.
    let scratch =
        std::env::temp_dir().join(format!("undra-bindgen-library-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let out = scratch.join("generated");
    let result = Command::new(env!("CARGO_BIN_EXE_undra"))
        .args(["bindgen", "-C"])
        .arg(&project)
        .arg("--library")
        .arg(&lib)
        .args(["--docs", "--out"])
        .arg(&out)
        .output()
        .expect("spawn `undra bindgen --library`");
    assert!(
        result.status.success(),
        "`undra bindgen --library` failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        !String::from_utf8_lossy(&result.stderr).contains("Building the core"),
        "a library that is given must not be built again: {stdout}"
    );

    let committed = project.join("generated");
    let written = files_below(&out);
    assert_eq!(
        written,
        files_below(&committed),
        "the trees list different files"
    );
    for file in written {
        // The one file that names where the tree is relative to the runtimes (the scratch directory is elsewhere).
        if file == Path::new("swift/Package.swift") {
            continue;
        }
        let actual = std::fs::read_to_string(out.join(&file)).expect("generated file");
        let expected = std::fs::read_to_string(committed.join(&file)).expect("committed file");
        assert_eq!(
            actual,
            expected,
            "{} differs from the committed tree",
            file.display()
        );
    }
    let _ = std::fs::remove_dir_all(&scratch);

    // `--library` and `--schema` are two ways to say where the schema comes from.
    let both = Command::new(env!("CARGO_BIN_EXE_undra"))
        .args(["bindgen", "-C"])
        .arg(&project)
        .arg("--library")
        .arg(&lib)
        .args(["--schema", "schema.json", "--check"])
        .output()
        .expect("spawn");
    assert!(!both.status.success());
    assert!(
        String::from_utf8_lossy(&both.stderr).contains("cannot be used with"),
        "{}",
        String::from_utf8_lossy(&both.stderr)
    );
}

#[test]
#[ignore = "runs a full `undra build`; run with --ignored (CI does)"]
fn a_loaded_core_keeps_its_schema_and_jni_exports() {
    let project = playground();
    assert!(
        project.join("undra.toml").is_file(),
        "the playground is missing at {}",
        project.display()
    );

    let status = Command::new(env!("CARGO_BIN_EXE_undra"))
        .args(["build", "-C"])
        .arg(&project)
        .args(["--platform", "host"])
        .status()
        .expect("spawn `undra build`");
    assert!(status.success(), "`undra build --platform host` failed");

    let lib =
        built_core(&project).expect("undra build did not leave build/host/libplayground_core.*");

    // The schema: reading it through the CLI's own loader also checks the library's self-reported
    // hash against its JSON. It must be the real schema, not the empty one a strip leaves behind.
    let schema = undra_cli::schema::load_from_library(&lib, NAMESPACE, "playground-core", true)
        .expect("the built core library is not a readable Undra core");
    let empty = schema.records.is_empty()
        && schema.enums.is_empty()
        && schema.objects.is_empty()
        && schema.functions.is_empty()
        && schema.ports.is_empty()
        && schema.queries.is_empty();
    assert!(
        !empty,
        "the loaded core reports an EMPTY schema: its `inventory` registrations were dead-stripped \
         (ADR-029). Restore the shim's non-incremental build."
    );
    assert!(
        schema.records.iter().any(|r| !r.docs.is_empty()),
        "the loaded core's schema has no doc comments: `undra_schema_json` must carry them \
         (SPEC 2.3), or `undra bindgen --docs` has nothing to write"
    );
    assert_ne!(
        schema.hash(),
        EMPTY_SCHEMA_HASH,
        "the loaded core reports the empty-schema hash {EMPTY_SCHEMA_HASH:#018x}: registrations stripped"
    );

    // ADR-044 changes the C ABI, not the wire: the schema hash is the one the committed bindings
    // were generated for.
    let ids = std::fs::read_to_string(project.join("generated/ts/src/ids.ts"))
        .expect("the playground's committed TypeScript bindings");
    let committed = format!("0x{:016x}n", schema.hash());
    assert!(
        ids.contains(&committed),
        "the loaded core's schema hash {committed} is not the one of the committed bindings"
    );

    // The exports: the table entry and `JNI_OnLoad` (whose registration the generated
    // `UndraCoreNative` relies on), nothing of C ABI version 1.
    let wanted = ["playground_core_undra_api", "JNI_OnLoad"];
    let present =
        undra_cli::schema::symbols_present(&lib, &wanted).expect("could not load the core");
    for (name, ok) in wanted.iter().zip(present) {
        assert!(
            ok,
            "the built core does not export `{name}`: the shim's `export_core!` entry points were \
             lost, so neither a host nor `System.loadLibrary` could reach the core (ADR-044)"
        );
    }
    let gone = [
        "undra_abi_version",
        "undra_schema_hash",
        "undra_init",
        "undra_call_sync",
        "Java_dev_undra_runtime_UndraNative_abiVersion",
        "Java_dev_undra_runtime_UndraNative_shutdown",
    ];
    let present = undra_cli::schema::symbols_present(&lib, &gone).expect("could not load the core");
    for (name, exported) in gone.iter().zip(present) {
        assert!(
            !exported,
            "the built core still exports `{name}`: a core exports one namespaced entry (ADR-044), \
             or two cores in one process would bind each other's symbols"
        );
    }
}
