#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
//!
//! # Where things live
//!
//! | Path | What |
//! |---|---|
//! | [`api`], [`error`], [`store`], [`port`], [`query`](macro@query), [`mutation`], [`migrate`] | the attribute macros (`undra-macros`) |
//! | [`persist`] | what a `#[undra::migrate]` hook works with: `DynValue`, `DynRecord`, `MigrateError` (ADR-037) |
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

pub use undra_macros::{api, error, migrate, mutation, port, query, store};
pub use undra_meta as meta;
pub use undra_ports as ports;
pub use undra_runtime as runtime;
/// Persisted values across app updates (ADR-037): the dynamic values a `#[undra::migrate]` hook
/// receives and returns, and its error.
pub use undra_runtime::persist;
pub use undra_signals as signals;
pub use undra_wire as wire;

pub mod query;

/// What an application core imports: `use undra::prelude::*;`.
///
/// It brings the reactive primitives (`Signal`, `Computed`, `Effect`, `txn`), the runtime
/// handles (`Ctx`, and `WeakCtx` with its `Gone` for anything that outlives a call, ADR-034),
/// the wire scalars a public signature may use (`Bytes`, `Uuid`, `Timestamp`, `Duration`;
/// `Handle` is the runtime's reference to an object instance and is not a schema type, so a
/// public signature cannot use it), the seven attribute macros
/// (`#[undra::api]` and friends are also reachable as `undra::api`, ...). It also brings
/// [`CtxQuery`](crate::query::CtxQuery), so `ctx.query()` and `ctx.mutate(..)` work, and
/// [`CtxPorts`](undra_ports::CtxPorts), so `ctx.http()`, `ctx.kv()` and the other standard
/// port accessors work. A migration hook's types (`DynValue`, `DynRecord`, `MigrateError`) come
/// with it.
pub mod prelude {
    pub use core::time::Duration;

    pub use undra_macros::{api, error, migrate, mutation, port, query, store};
    pub use undra_ports::CtxPorts;
    pub use undra_query::CtxQuery;
    pub use undra_runtime::persist::{DynRecord, DynValue, MigrateError};
    pub use undra_runtime::{Ctx, Gone, WeakCtx};
    pub use undra_signals::{Computed, Effect, Signal, txn};
    pub use undra_wire::{Bytes, Handle, Timestamp, Uuid};
}
