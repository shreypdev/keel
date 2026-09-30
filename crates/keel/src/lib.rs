#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
//!
//! # Where things live
//!
//! | Path | What |
//! |---|---|
//! | [`api`], [`error`], [`store`], [`port`], [`query`](macro@query), [`mutation`] | the attribute macros (`keel-macros`) |
//! | [`prelude`] | what an application core imports: signals, `Ctx`, the wire scalars, the macros |
//! | [`runtime`] | `keel-runtime`: `Runtime`, `Ctx`, dispatch, ports, the test runtime |
//! | [`signals`] | `keel-signals`: `Signal`, `Computed`, `Effect`, `txn`, `StoreCell` |
//! | [`wire`] | `keel-wire`: the binary codec |
//! | [`meta`] | `keel-meta`: the schema every language is generated from |
//! | [`query`](mod@query) | `keel-query`: the traits `#[keel::query]` and `#[keel::mutation]` implement, and `ctx.query()` / `ctx.mutate(..)` |
//! | [`ports`] | `keel-ports`: the standard ports (`Http`, `Kv`, `Clock`, ...), their records and the deterministic fakes |
//!
//! The code the macros generate names everything through `::keel::{wire, meta, runtime,
//! signals, query}` (SPEC 16.3), which is why an application depends on this crate alone.

pub use keel_macros::{api, error, mutation, port, query, store};
pub use keel_meta as meta;
pub use keel_ports as ports;
pub use keel_runtime as runtime;
pub use keel_signals as signals;
pub use keel_wire as wire;

pub mod query;

/// What an application core imports: `use keel::prelude::*;`.
///
/// It brings the reactive primitives (`Signal`, `Computed`, `Effect`, `txn`), the runtime
/// handle (`Ctx`), the wire scalars a public signature may use (`Bytes`, `Uuid`, `Timestamp`,
/// `Duration`; `Handle` is the runtime's reference to an object instance and is not a schema
/// type, so a public signature cannot use it) and the six attribute macros
/// (`#[keel::api]` and friends are also reachable as `keel::api`, ...). It also brings
/// [`CtxQuery`](crate::query::CtxQuery), so `ctx.query()` and `ctx.mutate(..)` work, and
/// [`CtxPorts`](keel_ports::CtxPorts), so `ctx.http()`, `ctx.kv()` and the other standard
/// port accessors work.
pub mod prelude {
    pub use core::time::Duration;

    pub use keel_macros::{api, error, mutation, port, query, store};
    pub use keel_ports::CtxPorts;
    pub use keel_query::CtxQuery;
    pub use keel_runtime::Ctx;
    pub use keel_signals::{Computed, Effect, Signal, txn};
    pub use keel_wire::{Bytes, Handle, Timestamp, Uuid};
}
