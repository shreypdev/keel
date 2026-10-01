//! Runs the generated TypeScript. The package of a golden case is compiled to
//! JavaScript with `tsc` and executed by Node against the real wire layer of
//! `@undra/runtime` and a fake core (`tests/fixtures/ts-run/*.mjs`), so the
//! encodings, the error mapping, the stores' patch handling and the port
//! adapters are checked by behaviour and not only by types. Skipped, with a
//! message on stderr, without `tsc` and `node`; `UNDRA_REQUIRE_TOOLCHAINS=1`
//! turns the skip into a failure.

mod common;

use std::process::Command;

use common::{
    install_ts_runtime, manifest_dir, on_path, scratch, skip, ts_runtime_declarations,
    ts_runtime_js, tsc,
};
use undra_bindgen::GeneratedFile;

/// Compiles the TypeScript of `case` to JavaScript and runs
/// `tests/fixtures/ts-run/<script>.mjs` against it.
fn run(
    case: &str,
    script: &str,
    args: &[&str],
    configure: impl FnOnce(&mut undra_bindgen::Generator),
) {
    if !on_path("node") {
        skip("node not found");
        return;
    }
    let (Some(declarations), Some(js)) = (ts_runtime_declarations(), ts_runtime_js()) else {
        skip("no TypeScript compiler (tsc) found");
        return;
    };
    let schema = common::case(case);
    let mut generator = common::generator_for(case, &schema);
    configure(&mut generator);
    let files = generator.typescript(&schema).unwrap();
    let root = scratch(&format!("ts-run-{case}-{script}-{}", args.join("-")));
    GeneratedFile::write_all(&files, &root).unwrap();
    install_ts_runtime(&root, declarations, Some(js));

    let mut compile = tsc().expect("tsc was found earlier");
    let output = compile
        .args(["-p"])
        .arg(root.join("tsconfig.json"))
        .args(["--declaration", "false", "--sourceMap", "false"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "compiling the generated TypeScript of `{case}` failed:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );

    let output = Command::new("node")
        .arg(manifest_dir().join(format!("tests/fixtures/ts-run/{script}.mjs")))
        .arg(&root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "the generated `{case}` package fails its execution test:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn the_playground_like_package_behaves() {
    run("full", "full", &[], |_| {});
}

#[test]
fn records_encode_every_number_type() {
    run("records", "records", &[], |_| {});
}

#[test]
fn records_with_js_numbers() {
    run("records", "records", &["js-number"], |g| {
        g.ts_js_number = true
    });
}

#[test]
fn enums_use_their_wire_indexes() {
    run("enums", "enums", &[], |_| {});
}

#[test]
fn errors_render_messages_and_survive_the_module_cycle() {
    run("errors", "errors", &["types-first"], |_| {});
    run("errors", "errors", &["errors-first"], |_| {});
}

#[test]
fn objects_map_calls_and_errors() {
    run("objects", "objects", &[], |_| {});
}

#[test]
fn stores_mirror_every_signal_type() {
    run("stores", "stores", &[], |_| {});
}

#[test]
fn standard_types_come_from_the_runtime() {
    run("stdlib", "stdlib", &[], |_| {});
}
