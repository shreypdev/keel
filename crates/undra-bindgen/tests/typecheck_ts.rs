//! Type-checks the generated TypeScript against the real wire layer and the
//! real standard types of `@undra/runtime` (its declarations are built from
//! source on every run) and the hand-written declaration of the base API in
//! `tests/fixtures/ts-base/index.d.ts`. Skipped, with a message on stderr,
//! when no TypeScript compiler is installed; `UNDRA_REQUIRE_TOOLCHAINS=1` turns
//! the skip into a failure.

mod common;

use common::{install_ts_runtime, scratch, skip, ts_runtime_declarations, tsc};
use undra_bindgen::GeneratedFile;

/// Writes `files` as a package next to the installed runtime and type-checks it
/// with the package's own `tsconfig.json`.
fn typecheck(name: &str, files: &[GeneratedFile]) -> Result<(), String> {
    let declarations = ts_runtime_declarations().expect("declarations were built");
    let root = scratch(&format!("ts-{name}"));
    GeneratedFile::write_all(files, &root).unwrap();
    install_ts_runtime(&root, declarations, None);
    let mut cmd = tsc().expect("tsc was found earlier");
    let output = cmd
        .args(["--noEmit", "-p"])
        .arg(root.join("tsconfig.json"))
        .output()
        .unwrap();
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

fn typecheck_case(case: &str) {
    if ts_runtime_declarations().is_none() {
        skip("no TypeScript compiler (tsc) found");
        return;
    }
    let schema = common::case(case);
    let generator = common::generator_for(case, &schema);
    let files = generator.typescript(&schema).unwrap();
    if let Err(diagnostics) = typecheck(case, &files) {
        panic!("the generated TypeScript of `{case}` does not type-check:\n{diagnostics}");
    }
}

macro_rules! ts_cases {
    ($($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                typecheck_case(stringify!($name));
            }
        )*
    };
}

ts_cases!(
    object_graph,
    callbacks,
    records,
    enums,
    errors,
    objects,
    stores,
    ports,
    queries,
    full,
    stdlib,
    recursive
);

#[test]
fn js_number_output_type_checks() {
    if ts_runtime_declarations().is_none() {
        skip("no TypeScript compiler (tsc) found");
        return;
    }
    let schema = common::case("records");
    let mut generator = common::generator_for("records", &schema);
    generator.ts_js_number = true;
    let files = generator.typescript(&schema).unwrap();
    if let Err(diagnostics) = typecheck("records-js-number", &files) {
        panic!("the js_number TypeScript does not type-check:\n{diagnostics}");
    }
}
