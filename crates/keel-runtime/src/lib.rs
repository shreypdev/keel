#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! The Keel runtime (SPEC 5, 6, 16.2): executor, core lock, object table, dispatch, ports,
//! timers, panic guard and snapshots. `keel-ffi`, the wasm shell and the transport server are
//! thin shells over the [`Runtime`] here.
//!
//! The runtime is dependency-light: no tokio, no async runtime crate, no `unsafe`. The
//! threading model as built is described in the crate's `docs/runtime-internals.md` (locks,
//! their order, cancellation, re-entrancy, shutdown); the short version:
//!
//! * One **core lock** ([`parking_lot::Mutex`]) is held while user code runs, either a
//!   dispatcher answering a host call on the caller's thread, or a batch of task polls on the
//!   `keel-core` thread. Whoever holds it *is* the core loop, which is what gives Keel its
//!   one-mutator semantics.
//! * Everything that must be reachable without the lock (wakers, port replies, timer fires,
//!   stream credit) has its own small lock and never takes the core lock.
//! * Host callbacks ([`Host::reply`], [`Host::change_set`], ...) may run with the core lock
//!   held; a host that calls back into the runtime from one of them is refused with
//!   `E_REENTRANT` rather than deadlocked.
//! * Nothing escapes as a panic: dispatchers, task polls, event subscribers, restore
//!   functions and destructors all run under a guard that turns a panic into a status 2 reply
//!   (message and backtrace), a level 5 log record and a poisoned-store flag.
//!
//! # For macro authors: what generated code calls
//!
//! A generated dispatcher (`keel_meta::DispatchFn`) receives the runtime as `&dyn Any`,
//! downcasts it to [`Runtime`] and returns `DispatchOutcome::new(DispatchResult::..)`. It
//! resolves receivers with [`Runtime::object`], stores new objects with
//! [`Runtime::insert_object`] or [`Runtime::insert_store`] (and encodes the returned
//! [`Handle`]`.0` as a `u64` in the `Ok` body of a constructor reply),
//! and reaches ports through [`Ctx::port_call`], [`port_call_sync`] and [`Ctx::rust_port`] /
//! [`Ctx::dyn_port`]. `#[keel::store]` submits one [`StoreRestorer`] per store type.
//!
//! # Deviations from SPEC 16.2
//!
//! * [`DispatchResult`] has a fifth variant, `BadRequest(String)`, for undecodable arguments
//!   and stale receivers (status 5 with a reason); the spec only has `Unknown`, which cannot
//!   express "the arguments did not decode".
//! * `StoreRestorer`, `InitHook`, [`Runtime::extension`], [`Runtime::new`] and the typed
//!   `insert_*`/`object` helpers are additions the spec implies but does not name.

pub use keel_meta;
pub use keel_meta::inventory;
pub use keel_signals;
pub use keel_wire;

mod blocking;
mod config;
mod ctx;
mod dispatch;
pub mod executor;
mod ext;
mod guard;
mod host;
mod lazy;
pub mod log;
mod object;
pub mod object_table;
mod ports;
mod runtime;
mod stats;
pub mod testing;
mod timer;

pub use config::{InitError, MODE_DEV, MODE_INPROC, RestoreError, RuntimeConfig};
pub use ctx::{Ctx, CtxScope};
pub use dispatch::{DispatchBytes, DispatchResult};
pub use ext::InitHook;
pub use host::{Host, PortCallOutcome};
pub use keel_meta::{DispatchCall, DispatchOutcome};
pub use keel_wire::Handle;
pub use lazy::{LazyList, LazyListInner};
pub use object::{
    AnyObject, CellFn, KeelObject, KeelObjectDyn, RestoreFn, StoreObject, StoreRestorer, plain,
    store,
};
pub use ports::{
    EventHandler, Events, Port, PortDispatch, PortDispatcher, PortError, PortFuture, Subscription,
    port_call_sync,
};
pub use runtime::Runtime;

/// Re-export so generated code can name the stream trait as `::keel::runtime::Stream`.
pub use futures_core::Stream;
