#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
//!
//! # Where things live
//!
//! | Path | What |
//! |---|---|
//! | [`api`], [`error`], [`store`], [`port`], [`callback`], [`query`](macro@query), [`mutation`], [`migrate`] | the attribute macros (`undra-macros`) |
//! | [`persist`] | what a `#[undra::migrate]` hook works with: `DynValue`, `DynRecord`, `MigrateError` (ADR-037) |
//! | [`prelude`] | what an application core imports: signals, `Ctx`, the wire scalars, the macros |
//! | [`runtime`] | `undra-runtime`: `Runtime`, `Ctx`, dispatch, ports, the test runtime |
//! | [`signals`] | `undra-signals`: `Signal`, `Computed`, `DerivedList`, `Lazy`, `Effect`, `txn`, `StoreCell` |
//! | [`wire`] | `undra-wire`: the binary codec |
//! | [`meta`] | `undra-meta`: the schema every language is generated from |
//! | [`query`](mod@query) | `undra-query`: the traits `#[undra::query]` and `#[undra::mutation]` implement, and `ctx.query()` / `ctx.mutate(..)` |
//! | [`ports`] | `undra-ports`: the standard ports (`Http`, `Kv`, `Clock`, ...), their records and the deterministic fakes |
//! | [`testing`] | `undra-testkit`: the test runtime, the fakes, `Seed`, `Harness`, and port `Recorder` / `Replayer` over the `undra.recording` format |
//!
//! The code the macros generate names everything through `::undra::{wire, meta, runtime,
//! signals, query}` (SPEC 16.3), which is why an application depends on this crate alone.
//!
//! # Newtypes, generic data types and leaf types (ADR-042)
//!
//! * **Newtypes.** `#[undra::api] pub struct UserId(pub Uuid);` (exactly one field) crosses as its
//!   inner type: the same bytes, a distinct type on every platform. It may be a map key when its
//!   inner type is (`HashMap<UserId, User>`), checked by the compiler; a newtype of `Decimal` is not.
//! * **Generic data types** are instantiated under a name:
//!   `#[undra::api(generic)] pub struct Page<T> { .. }` is a template that registers nothing, and
//!   `#[undra::api] pub type TodoPage = Page<Todo>;` registers `TodoPage` as a record like any other.
//!   Signatures spell the alias (`Page<Todo>` is E0002); the alias lives in the crate of its template.
//! * **`Decimal`** is an exact decimal number (`mantissa: i128`, `scale: u8`, at most 38), the type
//!   for money. [`Decimal::cmp_numeric`](wire::Decimal::cmp_numeric) compares by value; `==` is
//!   structural (`1.0` and `1.00` differ), so it is not a map key.
//! * **Leaf types of other crates**, behind features of this crate: `uuid` (`uuid::Uuid` as `Uuid`),
//!   `chrono` (`DateTime<Utc>` as `Timestamp`, `TimeDelta` as `Duration`), `time` (`OffsetDateTime`
//!   and `UtcDateTime` as `Timestamp`, `time::Duration` as `Duration`), `rust_decimal`
//!   (`rust_decimal::Decimal` as `Decimal`) and `bytes` (`bytes::Bytes` as `Bytes`). None of them
//!   enables a clock or randomness feature of its crate.
//!
//! **Precision.** `Timestamp` is milliseconds since the Unix epoch: a finer `chrono` or `time` value
//! truncates toward negative infinity on encode. `Duration` is non-negative nanoseconds: a negative
//! `chrono::TimeDelta` or `time::Duration` encodes as zero, one beyond `i64::MAX` nanoseconds (about
//! 292 years) as the largest. `time` values are normalised to UTC. A `chrono::DateTime` in another time
//! zone than `Utc` does not cross (E0001): convert it with `with_timezone(&Utc)`.

/// The macro a `#[undra::api(generic)]` template calls to register one instantiation (ADR-042).
/// Not written by hand.
#[doc(hidden)]
pub use undra_macros::__instantiate;
pub use undra_macros::{api, callback, error, migrate, mutation, port, query, store};
pub use undra_meta as meta;
pub use undra_ports as ports;
pub use undra_runtime as runtime;
/// Persisted values across app updates (ADR-037): the dynamic values a `#[undra::migrate]` hook
/// receives and returns, and its error.
pub use undra_runtime::persist;
pub use undra_signals as signals;
/// A list the platforms page through instead of mirroring (ADR-043): a store field
/// (`books: Lazy<Book>`) the host asks for one window of at a time.
pub use undra_signals::Lazy;
pub use undra_testkit as testing;
pub use undra_wire as wire;

pub mod query;

/// What an application core imports: `use undra::prelude::*;`.
///
/// It brings the reactive primitives (`Signal`, `Computed`, `DerivedList`, `Lazy`, `Effect`, `txn`), the runtime
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

    pub use undra_macros::{api, callback, error, migrate, mutation, port, query, store};
    pub use undra_ports::CtxPorts;
    pub use undra_query::CtxQuery;
    pub use undra_runtime::persist::{DynRecord, DynValue, MigrateError};
    pub use undra_runtime::{Ctx, Gone, WeakCtx};
    pub use undra_signals::{Computed, DerivedList, Effect, Lazy, Signal, txn};
    pub use undra_wire::{Bytes, Decimal, Handle, Timestamp, Uuid};
}
