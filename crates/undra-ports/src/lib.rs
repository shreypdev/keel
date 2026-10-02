#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
//!
//! # Where things are
//!
//! * The eleven port traits ([`Clock`], [`Rng`], [`Log`], [`Http`], [`Kv`], [`SecureStore`],
//!   [`Fs`], [`Timer`], [`Connectivity`], [`Lifecycle`], [`Diagnostics`]) and their records are re-exported at the
//!   crate root, together with what `#[undra::port]` generates for them: proxies
//!   (`HttpProxy`, ...), accessors ([`http`], [`kv`], [`clock`], ...), Rust-side dispatchers
//!   ([`KV_DISPATCHER`], ...: not registered, so a core links them only where a Rust
//!   implementation is bound with one through `Runtime::bind_dyn_port_with`, ADR-052) and,
//!   for the event ports, [`on_connectivity_changed`] / [`encode_connectivity_changed_event`] and
//!   [`on_lifecycle_changed`] / [`encode_lifecycle_changed_event`].
//! * [`CtxPorts`] adds `ctx.http()`, `ctx.kv()`, ... to [`Ctx`](undra_runtime::Ctx).
//! * [`run_background`] is the standard function every core has: a platform calls it with the
//!   window the OS granted (ADR-046), and it runs the background tasks of the runtime.
//! * [`fakes`] holds the deterministic fakes and [`fakes::install`].
//! * Opt-in ports, each behind a cargo feature of this crate and of `undra` (off by default, so a
//!   core that does not ask for them keeps its schema, hash and size; ADR-047, ADR-048):
//!   the module `ws` (feature `websocket`: the `WebSocket` port, `ws::WsConnection`), the module
//!   `sse` (feature `sse`: the `Sse` port, `sse::subscribe`) and the module `db` (feature `db`:
//!   the `Db` port over SQLite, `db::Database`). [`Backoff`] and [`next()`] serve the cores that
//!   reconnect and read them.
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

mod background;
mod backoff;
mod ctx_ext;
#[cfg(feature = "db")]
pub mod db;
pub mod fakes;
mod next;
#[cfg(any(feature = "websocket", feature = "sse", feature = "db"))]
mod owned;
mod ports;
mod records;
#[cfg(feature = "sse")]
pub mod sse;
#[cfg(feature = "websocket")]
pub mod ws;

pub use background::run_background;
pub use backoff::Backoff;
pub use ctx_ext::CtxPorts;
#[cfg(feature = "db")]
pub use db::{DB_DISPATCHER, Db, DbProxy, db};
pub use next::{Next, next};
pub use ports::*;
pub use records::*;
#[cfg(feature = "sse")]
pub use sse::{SSE_DISPATCHER, Sse, SseProxy, sse};
#[cfg(feature = "websocket")]
pub use ws::{WEB_SOCKET_DISPATCHER, WebSocket, WebSocketProxy, web_socket};
