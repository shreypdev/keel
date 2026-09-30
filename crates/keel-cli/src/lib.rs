#![deny(unsafe_code, missing_docs)]
#![deny(clippy::undocumented_unsafe_blocks)]
//! The `keel` command: the dev loop and the packaging of a Keel app.
//!
//! Keel apps are a Rust core plus a thin native shell per platform. This crate is the tool that
//! ties them together (`docs/SPEC.md` section 13):
//!
//! | Command | What it does |
//! |---|---|
//! | `keel init <name>` | scaffolds a project: a core crate with a working store, generated bindings, and an iOS, an Android and a web app that use them |
//! | `keel bindgen` | builds the core as a host library, loads it (`dlopen`), reads `keel_schema_json`, and writes Swift, Kotlin and TypeScript |
//! | `keel build` | builds the core for each platform: an XCFramework, `jniLibs/`, a wasm module; prints sizes |
//! | `keel dev` | serves the core over a WebSocket to running apps and rebuilds it when the code changes |
//! | `keel doctor` | checks the toolchains and SDKs, with the fix for each gap |
//! | `keel adopt` | adds a core to an existing app without touching the app's project files |
//!
//! # How a project is put together
//!
//! A project is a directory with a `keel.toml` ([`config`]). Its core is an ordinary library crate
//! that depends on `keel`. Everything that ships to a platform is built from two crates the CLI
//! generates under `target/keel/` (`shim`): the *shim*, which links the core and the C ABI
//! (`keel-ffi`) into a library named `keel_core`, and the *dev runner*, which links the core and
//! `keel-transport` into an executable. Keeping them out of the core means the core does not name
//! crate types, profiles or platform features, and `keel` can change them without touching user
//! code.
//!
//! # Errors
//!
//! Every failure is a [`error::CliError`]: a stable code (`C00NN`), what happened, why it matters
//! and what to do, in the shape of the macro diagnostics of SPEC 12. The codes are listed in
//! [`error::Code`].
//!
//! # Deviations from SPEC 13 and the constitution
//!
//! * R2 says `unsafe` lives in `keel-ffi` only; SPEC 13 has this crate `dlopen` the core, which
//!   cannot be done safely. The one module that does it (`schema`) is `#![allow(unsafe_code)]`
//!   with a `SAFETY` comment on every block; the rest of the crate denies `unsafe`.
//! * The library's `keel_schema_json` is the canonical JSON, which has no doc comments and no
//!   labels. `keel bindgen` adds the labels; `--docs` reads the full schema from the dev runner
//!   instead when the generated code should carry the Rust docs.

mod binary;
mod bindgen;
mod builds;
mod cargo;
mod cli;
mod commands;
pub mod config;
mod detect;
pub mod error;
mod fsutil;
mod names;
mod project;
mod render;
mod runner;
mod runtimes;
pub mod schema;
mod session;
mod shim;
mod sys;
mod templates;
mod toml_lite;
mod toolchain;
mod ui;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::commands::Env;
use crate::error::Result;
use crate::sys::RealSys;
use crate::ui::Ui;

/// Runs the CLI with `args` (the first is the program name) and returns the process exit code:
/// `0` on success, `1` for a failed command or a failed `keel doctor`, `2` for a bad command line.
///
/// Errors are printed to stderr in the form described in [`error`]; results go to stdout.
pub fn run<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            let code = if e.use_stderr() { 2 } else { 0 };
            let _ = e.print();
            return ExitCode::from(code);
        }
    };
    let ui = Ui::detect();
    let sys = RealSys;
    match dispatch(&cli, &sys, ui) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("{}", ui.error_text(&e));
            ExitCode::from(1)
        }
    }
}

/// Runs the parsed command; `Ok(false)` is a command that reported its own failure (`doctor`).
fn dispatch(cli: &Cli, sys: &dyn sys::Sys, ui: Ui) -> Result<bool> {
    let env = Env {
        sys,
        ui,
        project_dir: cli.project_dir.clone(),
    };
    match &cli.command {
        Command::Init(args) => commands::init::run(&env, args).map(|()| true),
        Command::Bindgen(args) => commands::bindgen::run(&env, args).map(|()| true),
        Command::Build(args) => commands::build::run(&env, args).map(|()| true),
        Command::Dev(args) => commands::dev::run(&env, args).map(|()| true),
        Command::Doctor(args) => commands::doctor::run(&env, args.platform.as_deref()),
        Command::Adopt(args) => commands::adopt::run(&env, args).map(|()| true),
    }
}
