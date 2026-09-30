#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Keel's standard ports (SPEC 8): the ten port traits and the records they exchange.

/// The path generated code names its dependencies through (SPEC 16.3). `keel-ports` cannot use
/// the `keel` facade (which depends on it), so the macros are pointed here with
/// `#[keel(crate = "crate::root")]`.
#[allow(unused_imports)]
mod root {
    pub use keel_meta as meta;
    pub use keel_runtime as runtime;
    pub use keel_runtime::keel_signals as signals;
    pub use keel_wire as wire;
}

mod ports;
mod records;

pub use ports::*;
pub use records::*;
