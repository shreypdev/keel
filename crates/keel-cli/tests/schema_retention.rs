//! Regression guard for ADR-029: a cdylib `keel build` produces keeps the app core's schema
//! registrations and keel-ffi's JNI exports when it is *loaded* (not linked against).
//!
//! The platform runtimes load `libkeel_core` with no link-time reference to it: `keel bindgen`
//! dlopens it for the schema, and the Kotlin runtime `System.loadLibrary`s it and binds the JNI
//! natives. An incremental build on macOS dead-strips the app core's `inventory::submit!` statics
//! (the schema, SPEC 2.4) and keel-ffi's `#[no_mangle]` JNI exports (SPEC 6.1) because they live in
//! dependency rlibs the linker prunes. This test builds the playground core through the real `keel`
//! binary and asserts, on the loaded library, that neither was stripped.
//!
//! `#[ignore]` because it runs a full `keel build` (compiles the core and the shim cdylib); run it
//! with `--ignored`, as CI does. It deliberately does not pin `CARGO_INCREMENTAL`: `keel build`
//! forces it off itself, so leaving the environment's default (incremental on, in a dev shell and
//! in CI) is what makes this a real regression test of the fix.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The hash of the canonical empty schema: what `keel_schema_hash` reports when the registrations
/// were dead-stripped (`keel_meta::Schema::new(..).hash()`).
const EMPTY_SCHEMA_HASH: u64 = 0x9875_4cbe_a76a_32b2;

fn playground() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/playground")
}

fn built_core(project: &Path) -> Option<PathBuf> {
    let host = project.join("build/host");
    for name in ["libkeel_core.dylib", "libkeel_core.so", "keel_core.dll"] {
        let candidate = host.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[test]
#[ignore = "runs a full `keel build`; run with --ignored (CI does)"]
fn a_loaded_core_keeps_its_schema_and_jni_exports() {
    let project = playground();
    assert!(
        project.join("keel.toml").is_file(),
        "the playground is missing at {}",
        project.display()
    );

    let status = Command::new(env!("CARGO_BIN_EXE_keel"))
        .args(["build", "-C"])
        .arg(&project)
        .args(["--platform", "host"])
        .status()
        .expect("spawn `keel build`");
    assert!(status.success(), "`keel build --platform host` failed");

    let lib = built_core(&project).expect("keel build did not leave build/host/libkeel_core.*");

    // The schema: reading it through the CLI's own loader also checks the library's self-reported
    // hash against its JSON. It must be the real schema, not the empty one a strip leaves behind.
    let schema = keel_cli::schema::load_from_library(&lib, "playground-core")
        .expect("the built core library is not a readable Keel core");
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
    assert_ne!(
        schema.hash(),
        EMPTY_SCHEMA_HASH,
        "the loaded core reports the empty-schema hash {EMPTY_SCHEMA_HASH:#018x}: registrations stripped"
    );

    // The JNI exports: `System.loadLibrary` must be able to bind them, and `JNI_OnLoad` must run.
    let wanted = [
        "keel_abi_version",
        "keel_schema_hash",
        "JNI_OnLoad",
        "Java_dev_keel_runtime_KeelNative_abiVersion",
    ];
    let present =
        keel_cli::schema::symbols_present(&lib, &wanted).expect("could not load the core");
    for (name, ok) in wanted.iter().zip(present) {
        assert!(
            ok,
            "the built core does not export `{name}`: keel-ffi's exports were dead-stripped from the \
             loaded library (ADR-029), so `System.loadLibrary` would fail with UnsatisfiedLinkError"
        );
    }
}
