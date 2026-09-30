//! The `keel` executable: a thin shell over [`keel_cli::run`].

use std::process::ExitCode;

fn main() -> ExitCode {
    keel_cli::run(std::env::args_os())
}
