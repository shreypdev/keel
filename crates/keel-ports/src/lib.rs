#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
//!
//! # Where things are
//!
//! * The ten port traits ([`Clock`], [`Rng`], [`Log`], [`Http`], [`Kv`], [`SecureStore`],
//!   [`Fs`], [`Timer`], [`Connectivity`], [`Lifecycle`]) and their records are re-exported at the
//!   crate root, together with what `#[keel::port]` generates for them: proxies
//!   (`HttpProxy`, ...), accessors ([`http`], [`kv`], [`clock`], ...), Rust-side dispatchers and,
//!   for the event ports, [`on_connectivity_changed`] / [`encode_connectivity_changed_event`] and
//!   [`on_lifecycle_changed`] / [`encode_lifecycle_changed_event`].
//! * [`fakes`] holds the deterministic fakes and [`fakes::install`].
//!
//! # Calling a port from core code
//!
//! ```no_run
//! use keel_ports::{Clock, Http, HttpError, HttpRequest, NetKind, on_connectivity_changed};
//! use keel_runtime::Ctx;
//!
//! async fn refresh(ctx: &Ctx, url: &str) -> Result<usize, HttpError> {
//!     // The Rust binding if one is installed (a fake), else a proxy to the platform.
//!     let started = keel_ports::clock(ctx).monotonic_ns();
//!     let response = keel_ports::http(ctx).request(HttpRequest::get(url)).await?;
//!     let _elapsed_ns = keel_ports::clock(ctx).monotonic_ns() - started;
//!     Ok(response.body.len())
//! }
//!
//! fn watch_network(ctx: &Ctx) {
//!     // Event ports are subscribed to; keep the subscription for the life of the runtime.
//!     on_connectivity_changed(ctx, |online: bool, kind: NetKind| {
//!         let _ = (online, kind); // e.g. refetch when back online
//!     })
//!     .detach();
//! }
//! ```

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

pub mod fakes;
mod ports;
mod records;

pub use ports::*;
pub use records::*;
