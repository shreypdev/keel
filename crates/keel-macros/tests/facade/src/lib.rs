//! A test-only stand-in for the `keel` facade.
//!
//! `keel-macros` generates code that names `::keel::wire`, `::keel::meta`,
//! `::keel::runtime`, `::keel::signals` and `::keel::query` (SPEC 16.3). keel-runtime and
//! keel-signals are written in parallel, so the macro tests run against this crate instead:
//! `wire` and `meta` are the real `keel-wire` and `keel-meta`; `runtime`, `signals` and
//! `query` are small, deliberately naive implementations of exactly the names the macros
//! reference (the list is in the keel-macros report). They are enough to *execute* generated
//! dispatchers, proxies and restore functions, which is the point: the macros are tested by
//! running their output, not only by reading it.
//!
//! Nothing here is meant to be efficient or complete. When the real crates land, the
//! `keel` dev-dependency of keel-macros is repointed at the real facade and the same tests
//! run against it.

#![allow(clippy::type_complexity)] // the boxed futures and streams mirror the real contract

pub use keel_macros::{api, error, mutation, port, query, store};
pub use keel_meta as meta;
pub use keel_wire as wire;

/// A second path to the same modules, to exercise `#[keel(crate = "::keel::rooted")]`.
pub mod rooted {
    pub use crate::{meta, query, runtime, signals, wire};
}

/// The names the `keel::prelude` re-exports (SPEC 16.2).
pub mod prelude {
    pub use crate::runtime::Ctx;
    pub use crate::signals::{Computed, Signal};
    pub use core::time::Duration;
    pub use keel_macros::{api, error, mutation, port, query, store};
    pub use keel_wire::{Bytes, Timestamp, Uuid};
}

pub mod query;
pub mod runtime;
pub mod signals;
pub mod testing;
