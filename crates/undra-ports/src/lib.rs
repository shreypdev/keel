#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
//!
//! # Where things are
//!
//! * The ten port traits ([`Clock`], [`Rng`], [`Log`], [`Http`], [`Kv`], [`SecureStore`],
//!   [`Fs`], [`Timer`], [`Connectivity`], [`Lifecycle`]) and their records are re-exported at the
//!   crate root, together with what `#[undra::port]` generates for them: proxies
//!   (`HttpProxy`, ...), accessors ([`http`], [`kv`], [`clock`], ...), Rust-side dispatchers and,
//!   for the event ports, [`on_connectivity_changed`] / [`encode_connectivity_changed_event`] and
//!   [`on_lifecycle_changed`] / [`encode_lifecycle_changed_event`].
//! * [`CtxPorts`] adds `ctx.http()`, `ctx.kv()`, ... to [`Ctx`](undra_runtime::Ctx).
//! * [`fakes`] holds the deterministic fakes and [`fakes::install`].
//!
//! # Calling a port from core code
//!
//! ```no_run
//! use undra_ports::{Clock, Http, HttpError, HttpRequest, NetKind, on_connectivity_changed};
//! use undra_runtime::Ctx;
//!
//! async fn refresh(ctx: &Ctx, url: &str) -> Result<usize, HttpError> {
//!     // The Rust binding if one is installed (a fake), else a proxy to the platform.
//!     let started = undra_ports::clock(ctx).monotonic_ns();
//!     let response = undra_ports::http(ctx).request(HttpRequest::get(url)).await?;
//!     let _elapsed_ns = undra_ports::clock(ctx).monotonic_ns() - started;
//!     Ok(response.body.len())
//! }
//!
//! fn watch_network(ctx: &Ctx) {
//!     // Event ports are subscribed to; keep the subscription for the life of the runtime. The
//!     // subscriber is given the runtime's `Ctx`: use it rather than capture one (ADR-034).
//!     on_connectivity_changed(ctx, |_ctx: &Ctx, online: bool, kind: NetKind| {
//!         let _ = (online, kind); // e.g. refetch when back online
//!     })
//!     .detach();
//! }
//! ```

/// The path generated code names its dependencies through (SPEC 16.3). `undra-ports` cannot use
/// the `undra` facade (which depends on it), so the macros are pointed here with
/// `#[undra(crate = "crate::root")]`. Records, errors and ports name `wire`, `meta` and `runtime`.
mod root {
    pub use undra_meta as meta;
    pub use undra_runtime as runtime;
    pub use undra_wire as wire;
}

mod ctx_ext;
pub mod fakes;
mod ports;
mod records;

pub use ctx_ext::CtxPorts;
pub use ports::*;
pub use records::*;
