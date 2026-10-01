//! The `undra` executable: a thin shell over [`undra_cli::run`].

use std::process::ExitCode;

fn main() -> ExitCode {
    undra_cli::run(std::env::args_os())
}
