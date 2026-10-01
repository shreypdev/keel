//! The web build: the shim as a wasm module (docs/SPEC.md 7).
//!
//! Always the `release-wasm` profile (`opt-level` z or s, LTO, `panic = "abort"`): a debug wasm
//! module is many times larger and is not what the size budget describes. `wasm-opt -Oz` runs
//! afterwards when binaryen is installed. The result is checked for the exports the TypeScript
//! runtime needs, so a wrong build fails here and not as a blank page.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::binary::{wasm_exports, wasm_problems};
use crate::cargo::{Build, Profile};
use crate::error::{CliError, Code, Result};
use crate::fsutil::{copy_file, create_dir_all, size_of};
use crate::session::Session;

use super::Artifact;

/// The Rust target of the web build.
pub const TRIPLE: &str = "wasm32-unknown-unknown";

/// Features wasm-opt must accept: what rustc's wasm32 target emits by default.
const WASM_OPT_FEATURES: &[&str] = &[
    "--enable-bulk-memory",
    "--enable-nontrapping-float-to-int",
    "--enable-sign-ext",
    "--enable-mutable-globals",
    "--enable-multivalue",
    "--enable-reference-types",
];

/// Builds `build/web/undra_core.wasm`.
///
/// # Errors
///
/// See [`crate::cargo::Cargo::build_library`]; `C0004` when the module lacks the exports the
/// runtime needs.
pub fn build(session: &Session<'_>) -> Result<Vec<Artifact>> {
    let manifest = session.shim_manifest()?;
    let files = session.cargo().build_library(&Build {
        manifest,
        target_dir: session.target_dir()?,
        triple: Some(TRIPLE.to_owned()),
        profile: Profile::ReleaseWasm,
        crate_type: "cdylib",
        features: Vec::new(),
        env: Vec::new(),
        lib_name: crate::shim::shim_lib_name(&session.project.root),
        rustc_args: Vec::new(),
    })?;
    let built = files
        .iter()
        .find(|f| f.extension().is_some_and(|e| e == "wasm"))
        .cloned()
        .ok_or_else(|| {
            CliError::new(
                Code::ToolFailed,
                "cargo built the core for wasm but produced no .wasm file",
                "the web build expects a cdylib for wasm32-unknown-unknown",
                "run `cargo clean` and try again; if it persists this is a bug in undra-cli",
            )
        })?;

    let out_dir = session.project.build_dir().join("web");
    create_dir_all(&out_dir)?;
    let out = out_dir.join("undra_core.wasm");
    let raw_size = size_of(&built);

    let mut note = None;
    match session.toolchain.which(session.sys, "wasm-opt") {
        Some(wasm_opt) => {
            session.ui.step("Optimizing with wasm-opt -Oz");
            if run_wasm_opt(&wasm_opt, &built, &out) {
                note = Some(format!(
                    "{} before wasm-opt",
                    crate::fsutil::human_size(raw_size)
                ));
            } else {
                session.ui.warn(
                    "wasm-opt failed on this module; keeping the unoptimized build (run it by hand to see why)",
                );
                copy_file(&built, &out)?;
            }
        }
        None => {
            copy_file(&built, &out)?;
            session.ui.warn(
                "wasm-opt (binaryen) is not installed, so the module is not shrunk further; `brew install binaryen` or `npm i -g wasm-opt` saves about 10-20%",
            );
        }
    }

    let bytes = std::fs::read(&out).map_err(|e| CliError::io("read", &out, &e))?;
    let exports = wasm_exports(&bytes).map_err(|why| {
        CliError::new(
            Code::ToolFailed,
            format!("{} is not a readable wasm module: {why}", out.display()),
            "the web build has to produce a module the TypeScript runtime can instantiate",
            "run the command again; if it persists run `wasm-opt --version` and `undra doctor`",
        )
    })?;
    let problems = wasm_problems(&exports);
    if !problems.is_empty() {
        return Err(CliError::new(
            Code::ToolFailed,
            format!(
                "{} is not a usable Undra core: {}",
                out.display(),
                problems.join("; ")
            ),
            "the web app loads this file with `UndraCore.load({ mode: \"wasm-main\", wasm })`",
            "check that the shim links undra-ffi (run `undra build --platform host` and `undra bindgen`), then rebuild",
        ));
    }

    let gzip = gzip_size(&out);
    let note = match (note, gzip) {
        (Some(n), Some(g)) => Some(format!("gzip {}, {n}", crate::fsutil::human_size(g))),
        (None, Some(g)) => Some(format!("gzip {}", crate::fsutil::human_size(g))),
        (n, None) => n,
    };
    Ok(vec![Artifact {
        label: "web wasm".to_owned(),
        size: size_of(&out),
        path: out,
        budget: Some("120 KB gzip (hello world)".to_owned()),
        note,
    }])
}

/// Runs `wasm-opt -Oz` from `input` to `output`; whether it succeeded.
fn run_wasm_opt(wasm_opt: &Path, input: &Path, output: &Path) -> bool {
    Command::new(wasm_opt)
        .arg("-Oz")
        .args(WASM_OPT_FEATURES)
        .args(["--strip-debug", "--strip-producers"])
        .arg(input)
        .arg("-o")
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .is_ok_and(|s| s.success())
}

/// The size of `file` gzipped at level 9, using the system `gzip`; `None` when it is not there.
fn gzip_size(file: &Path) -> Option<u64> {
    let output = Command::new("gzip")
        .args(["-9", "-c"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then_some(output.stdout.len() as u64)
}
