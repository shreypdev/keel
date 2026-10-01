#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
//!
//! # Where things live
//!
//! | Path | What |
//! |---|---|
//! | [`api`], [`error`], [`store`], [`port`], [`query`](macro@query), [`mutation`] | the attribute macros (`undra-macros`) |
//! | [`prelude`] | what an application core imports: signals, `Ctx`, the wire scalars, the macros |
//! | [`runtime`] | `undra-runtime`: `Runtime`, `Ctx`, dispatch, ports, the test runtime |
//! | [`signals`] | `undra-signals`: `Signal`, `Computed`, `Effect`, `txn`, `StoreCell` |
//! | [`wire`] | `undra-wire`: the binary codec |
//! | [`meta`] | `undra-meta`: the schema every language is generated from |
//! | [`query`](mod@query) | `undra-query`: the traits `#[undra::query]` and `#[undra::mutation]` implement, and `ctx.query()` / `ctx.mutate(..)` |
//! | [`ports`] | `undra-ports`: the standard ports (`Http`, `Kv`, `Clock`, ...), their records and the deterministic fakes |
//!
//! The code the macros generate names everything through `::undra::{wire, meta, runtime,
//! signals, query}` (SPEC 16.3), which is why an application depends on this crate alone.

pub use undra_macros::{api, error, mutation, port, query, store};
pub use undra_meta as meta;
pub use undra_ports as ports;
pub use undra_runtime as runtime;
pub use undra_signals as signals;
pub use undra_wire as wire;

pub mod query;

/// What an application core imports: `use undra::prelude::*;`.
///
/// It brings the reactive primitives (`Signal`, `Computed`, `Effect`, `txn`), the runtime
/// handle (`Ctx`), the wire scalars a public signature may use (`Bytes`, `Uuid`, `Timestamp`,
/// `Duration`; `Handle` is the runtime's reference to an object instance and is not a schema
/// type, so a public signature cannot use it) and the six attribute macros
/// (`#[undra::api]` and friends are also reachable as `undra::api`, ...). It also brings
/// [`CtxQuery`](crate::query::CtxQuery), so `ctx.query()` and `ctx.mutate(..)` work, and
/// [`CtxPorts`](undra_ports::CtxPorts), so `ctx.http()`, `ctx.kv()` and the other standard
/// port accessors work.
pub mod prelude {
    pub use core::time::Duration;

    pub use undra_macros::{api, error, mutation, port, query, store};
    pub use undra_ports::CtxPorts;
    pub use undra_query::CtxQuery;
    pub use undra_runtime::Ctx;
    pub use undra_signals::{Computed, Effect, Signal, txn};
    pub use undra_wire::{Bytes, Handle, Timestamp, Uuid};
}
