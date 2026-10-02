//! The web build: the shim as a wasm module (docs/SPEC.md 7).
//!
//! Always the `release-wasm` profile (`opt-level` z or s, LTO, `panic = "abort"`): a debug wasm
//! module is many times larger and is not what the size budget describes. `wasm-opt -Oz` runs
//! afterwards when binaryen is installed. The result is checked for the exports the TypeScript
//! runtime needs, so a wrong build fails here and not as a blank page.
//!
//! **Symbols (ADR-046).** The release profile keeps the DWARF line tables and the names of the core.
//! One `wasm-opt` run (`-Oz -g`) over the module *without its DWARF* makes the optimised module
//! with its names, and prints its function map (`--print-function-map`); the module that ships is
//! that module with its name section removed and nothing else changed, so a `wasm-function[i]:0x…`
//! of a production stack trace names the same function and the same byte offset in both, and the
//! shipped module is not larger than the one `-Oz --strip-debug --strip-producers` made before.
//! (The DWARF stays out of that run on purpose: when `wasm-opt` keeps DWARF it skips every pass that
//! cannot update it, and the module grows 5 to 7%, so a debug module with DWARF and a shipped module
//! of the same code cannot both exist.) A second run, over the module with its DWARF, makes the one
//! debuggers read:
//!
//! | File | What |
//! |---|---|
//! | `build/web/<ns>.wasm` | what ships: no names, no DWARF |
//! | `build/symbols/web/<ns>.debug.wasm` | the same code with its names: where every `wasm-function[i]:0x…` of a production stack trace resolves |
//! | `build/symbols/web/<ns>.wasm.functions.txt` | `index:name` per function of that module |
//! | `build/symbols/web/<ns>.dwarf.wasm` | the module optimised with its DWARF line tables kept (some passes skipped: other code, bigger): what Chrome's DevTools reads for breakpoints in `.rs` files, and what `vite dev` serves; `undra symbolicate` takes a function's definition line from it |
//!
//! `--no-symbols` runs the old pipeline (`-Oz --strip-debug --strip-producers`, no debug info).

use std::path::Path;
use std::process::{Command, Stdio};

use crate::binary::{wasm_exports, wasm_problems};
use crate::cargo::{Build, Profile};
use crate::error::{CliError, Code, Result};
use crate::fsutil::{copy_file, create_dir_all, size_of};
use crate::session::Session;
use crate::symbols::{Entry, Format, Symbols, sha256, slash_relative, wasm};

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

/// Builds `build/web/<namespace>.wasm` (ADR-044: a wasm module is its own namespace, so only the
/// file is named after it; its exports keep their SPEC 7 names).
///
/// # Errors
///
/// See [`crate::cargo::Cargo::build_library`]; `C0004` when the module lacks the exports the
/// runtime needs.
pub fn build(session: &Session<'_>, symbols: &Symbols<'_, '_>) -> Result<Vec<Artifact>> {
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
        cargo_config: symbols.cargo_config(Profile::ReleaseWasm, false),
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
    let namespace = session.namespace()?;
    let out = out_dir.join(format!("{namespace}.wasm"));
    let raw_size = size_of(&built);
    let symbol_dir = symbols.dir().join("web");
    let debug_module = symbol_dir.join(format!("{namespace}.debug.wasm"));
    let dwarf_module = symbol_dir.join(format!("{namespace}.dwarf.wasm"));
    let function_map = symbol_dir.join(format!("{namespace}.wasm.functions.txt"));

    let mut note = None;
    let mut wrote_symbols = false;
    match session.toolchain.which(session.sys, "wasm-opt") {
        Some(wasm_opt) if symbols.enabled => {
            session.ui.step(
                "Optimizing with wasm-opt -Oz (the shipped module, its names and the function map in one run; a second keeps the DWARF)",
            );
            create_dir_all(&symbol_dir)?;
            match optimise_with_symbols(
                session,
                &wasm_opt,
                &built,
                &out,
                &Outputs {
                    debug: &debug_module,
                    dwarf: &dwarf_module,
                    map: &function_map,
                },
            ) {
                Ok(()) => {
                    wrote_symbols = true;
                    note = Some(format!(
                        "{} before wasm-opt",
                        crate::fsutil::human_size(raw_size)
                    ));
                }
                Err(why) => {
                    session.ui.warn(&format!(
                        "wasm-opt failed on this module ({why}); keeping the unoptimized build (run it by hand to see why)"
                    ));
                    unoptimized(
                        &built,
                        &out,
                        &Outputs {
                            debug: &debug_module,
                            dwarf: &dwarf_module,
                            map: &function_map,
                        },
                    )?;
                    wrote_symbols = true;
                }
            }
        }
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
            session.ui.warn(
                "wasm-opt (binaryen) is not installed, so the module is not shrunk further; `brew install binaryen` or `npm i -g wasm-opt` saves about 10-20%",
            );
            if symbols.enabled {
                create_dir_all(&symbol_dir)?;
                unoptimized(
                    &built,
                    &out,
                    &Outputs {
                        debug: &debug_module,
                        dwarf: &dwarf_module,
                        map: &function_map,
                    },
                )?;
                wrote_symbols = true;
            } else {
                copy_file(&built, &out)?;
            }
        }
    }
    if !symbols.enabled {
        symbols.forget("web")?;
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

    let mut artifacts = Vec::new();
    if wrote_symbols {
        let identity = symbols
            .identity_from(|| schema_hash_with_node(session, &out))?
            .clone();
        let relative = |file: &Path| file.is_file().then(|| slash_relative(&symbols.dir(), file));
        let hash = sha256::hex(&bytes);
        symbols.record(Entry {
            platform: "web".to_owned(),
            namespace: identity.namespace,
            core_version: identity.core_version,
            schema_hash: identity.schema_hash,
            arch: "wasm32".to_owned(),
            format: Format::Wasm,
            image_id: Some(hash.clone()),
            sha256: hash,
            shipped: slash_relative(&session.project.build_dir(), &out),
            shipped_bytes: bytes.len() as u64,
            symbols: relative(&debug_module),
            function_map: relative(&function_map),
            dwarf: relative(&dwarf_module),
        })?;
        artifacts.push(Artifact {
            label: "symbols web debug wasm".to_owned(),
            size: size_of(&debug_module),
            path: debug_module.clone(),
            budget: None,
            note: Some("the shipped code with its names, not shipped".to_owned()),
        });
        if dwarf_module.is_file() {
            artifacts.push(Artifact {
                label: "symbols web dwarf wasm".to_owned(),
                size: size_of(&dwarf_module),
                path: dwarf_module.clone(),
                budget: None,
                note: Some("DWARF line tables, for debuggers, not shipped".to_owned()),
            });
        }
    }

    let gzip = gzip_size(&out);
    let note = match (note, gzip) {
        (Some(n), Some(g)) => Some(format!("gzip {}, {n}", crate::fsutil::human_size(g))),
        (None, Some(g)) => Some(format!("gzip {}", crate::fsutil::human_size(g))),
        (n, None) => n,
    };
    artifacts.insert(
        0,
        Artifact {
            label: "web wasm".to_owned(),
            size: size_of(&out),
            path: out,
            budget: Some("120 KB gzip (hello world)".to_owned()),
            note,
        },
    );
    Ok(artifacts)
}

/// The schema hash of the module at `wasm`, asked of the module itself under `node` (see
/// [`wasm::SCHEMA_HASH_SCRIPT`]); `None` without `node` or when the module does not answer.
fn schema_hash_with_node(session: &Session<'_>, wasm: &Path) -> Option<u64> {
    let node = session.toolchain.which(session.sys, "node")?;
    let out = session.sys.run(
        &node,
        &["-e", wasm::SCHEMA_HASH_SCRIPT, &wasm.display().to_string()],
        &session.toolchain.env_pairs(),
    )?;
    if !out.success {
        return None;
    }
    u64::from_str_radix(out.stdout.trim().trim_start_matches("0x"), 16).ok()
}

/// Where the symbol outputs of the web build go.
struct Outputs<'a> {
    /// The shipped code with its names.
    debug: &'a Path,
    /// The module optimised with its DWARF kept.
    dwarf: &'a Path,
    /// The function map of the debug module.
    map: &'a Path,
}

/// The module that ships: `module` (the optimised module with its names) without its debug and
/// name sections, and nothing else changed.
fn write_shipped(module: &Path, shipped: &Path) -> Result<()> {
    let bytes = std::fs::read(module).map_err(|e| CliError::io("read", module, &e))?;
    let stripped = wasm::strip_debug_sections(&bytes).map_err(|why| {
        CliError::new(
            Code::ToolFailed,
            format!("{} is not a readable wasm module: {why}", module.display()),
            "the shipped module is the debug module without its name section",
            "run the command again; if it persists run `wasm-opt --version` and `undra doctor`",
        )
    })?;
    std::fs::write(shipped, stripped).map_err(|e| CliError::io("write", shipped, &e))
}

/// Without a working `wasm-opt`: the module Cargo made is the DWARF module, its copy without DWARF
/// is the debug module, and the shipped module is that without its names. The addresses agree
/// (nothing was optimised); there is no function map, the debug module's name section has the names.
fn unoptimized(built: &Path, shipped: &Path, outputs: &Outputs<'_>) -> Result<()> {
    let bytes = std::fs::read(built).map_err(|e| CliError::io("read", built, &e))?;
    let names_only = wasm::strip_dwarf_sections(&bytes).map_err(|why| {
        CliError::new(
            Code::ToolFailed,
            format!("{} is not a readable wasm module: {why}", built.display()),
            "the web build has to produce a module the TypeScript runtime can instantiate",
            "run the command again",
        )
    })?;
    std::fs::write(outputs.debug, names_only)
        .map_err(|e| CliError::io("write", outputs.debug, &e))?;
    copy_file(built, outputs.dwarf)?;
    let _ = std::fs::remove_file(outputs.map);
    write_shipped(outputs.debug, shipped)
}

/// The two `wasm-opt` runs of a build with symbols: the module without its DWARF (names kept) makes
/// the debug module and the function map, the shipped module is the debug module without its names;
/// the module with its DWARF makes the debuggers' one. The second is optional (a failure there is
/// said, not fatal).
fn optimise_with_symbols(
    session: &Session<'_>,
    wasm_opt: &Path,
    built: &Path,
    shipped: &Path,
    outputs: &Outputs<'_>,
) -> std::result::Result<(), String> {
    let bytes =
        std::fs::read(built).map_err(|e| format!("cannot read {}: {e}", built.display()))?;
    let names_only = wasm::strip_dwarf_sections(&bytes)?;
    let input = built.with_extension("names.wasm");
    std::fs::write(&input, names_only)
        .map_err(|e| format!("cannot write {}: {e}", input.display()))?;
    let map = run_wasm_opt_with_symbols(wasm_opt, &input, outputs.debug)
        .ok_or("the run over the module without DWARF failed")?;
    std::fs::write(outputs.map, map)
        .map_err(|e| format!("cannot write {}: {e}", outputs.map.display()))?;
    write_shipped(outputs.debug, shipped).map_err(|e| e.what)?;
    if !run_wasm_opt_keeping_dwarf(wasm_opt, built, outputs.dwarf) {
        let _ = std::fs::remove_file(outputs.dwarf);
        session.ui.warn(
            "wasm-opt failed on the module with its DWARF: there is no DWARF module (no breakpoints in .rs files in the browser, no definition lines in `undra symbolicate`)",
        );
    }
    let _ = std::fs::remove_file(&input);
    Ok(())
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

/// The `wasm-opt` run over the module without its DWARF: `-Oz -g` writes the optimised module with
/// its names to `debug_module`, and `--print-function-map` (after the optimisation, so it names the
/// final function indexes) prints the map, which is returned. `None` when it fails.
fn run_wasm_opt_with_symbols(wasm_opt: &Path, input: &Path, debug_module: &Path) -> Option<String> {
    let output = Command::new(wasm_opt)
        .args(wasm_opt_symbol_args())
        .arg(input)
        .arg("-o")
        .arg(debug_module)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The `wasm-opt` flags of the run that makes the debug module: the optimisation level and
/// features of the plain build, `-g` to keep the names, and the function map printed last.
#[must_use]
pub fn wasm_opt_symbol_args() -> Vec<&'static str> {
    let mut args = vec!["-Oz"];
    args.extend(WASM_OPT_FEATURES);
    args.extend(["-g", "--print-function-map"]);
    args
}

/// The `wasm-opt` run that keeps the DWARF (`-Oz -g` over a module that has it): what a debugger
/// reads. It skips the passes that cannot update DWARF, so its code is not the shipped one.
fn run_wasm_opt_keeping_dwarf(wasm_opt: &Path, input: &Path, output: &Path) -> bool {
    Command::new(wasm_opt)
        .arg("-Oz")
        .args(WASM_OPT_FEATURES)
        .arg("-g")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_that_makes_the_shipped_code_keeps_names_and_prints_the_map_after_the_optimisation() {
        let args = wasm_opt_symbol_args();
        assert_eq!(args[0], "-Oz");
        let g = args.iter().position(|a| *a == "-g").unwrap();
        let map = args
            .iter()
            .position(|a| *a == "--print-function-map")
            .unwrap();
        assert!(g < map, "passes run in command-line order: {args:?}");
        for feature in WASM_OPT_FEATURES {
            assert!(args.contains(feature), "{feature}");
        }
        // The shipped module is not stripped by this run: that is done to its output, so the two
        // modules are one optimisation.
        assert!(!args.iter().any(|a| a.starts_with("--strip")), "{args:?}");
    }
}
