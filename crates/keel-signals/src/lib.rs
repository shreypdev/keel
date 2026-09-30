#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
//!
//! # Threading
//!
//! Everything is `Send + Sync`, and concurrent writes from several threads are memory-safe and
//! deadlock-free. The Keel runtime additionally serialises all mutation under its core lock
//! (SPEC 5.1), and signal writes are only meant to happen on the core (see
//! [`set_write_checker`]). A transaction belongs to the thread that opened it: it is committed
//! by that thread, and the sink, effects and computed closures it triggers run on that thread.
//!
//! What this crate guarantees on its own, without the core lock: the change-sets of **one
//! store** reach the sink one at a time, in the order they were built, with increasing
//! transaction ids. Each store has a delivery lock that a commit holds from the moment it
//! claims the store's dirty slots until the sink has returned (see [`ChangeSink`]: the sink is
//! called under it, so a sink must not wait for another thread that writes the same store).
//! [`StoreCell::observe_and_deliver`] takes the same lock for an observe, so the values an
//! observer hands the host cannot be overtaken by a commit of the same store.
//! Nothing orders the change-sets of different stores against each other, and the writes of two
//! threads do not form one transaction: if a slot is already dirty in a transaction another
//! thread has open, a write from this thread is delivered with that thread's transaction, not
//! before the write returns.
//!
//! # Panics
//!
//! A panic inside [`txn`], a computed closure, an effect, an encoder or a sink leaves the
//! thread's transaction state consistent (see [`txn`](crate::txn())). The panic is re-raised
//! from the outermost write once the commit has finished, so a write can panic if something it
//! triggered did. If encoding or delivering a store's change-set panics, that change-set is
//! abandoned (never half-sent) and the claim is undone: the slots it covered are remembered and
//! sent again, as full values, by the next commit that touches the store (or dropped once the
//! host observes them again), so the host never keeps values the core has moved on from. A
//! computed that panics on every evaluation therefore holds back the store's later change-sets
//! until it recovers. `observe` is transactional the same way: if it panics, no target stays
//! observed because of it and no partial entries are produced.

mod computed;
mod context;
mod deps;
mod effect;
mod error;
mod graph;
mod signal;
mod sink;
mod store;
pub mod testing;
mod txn;
mod value;

pub use computed::Computed;
pub use context::{clear_write_checker, set_write_checker};
pub use deps::{Dep, Deps};
pub use effect::Effect;
pub use error::SignalsError;
pub use signal::Signal;
pub use sink::{ChangeSink, clear_sink, set_sink, with_sink};
pub use store::{ALL_SIGNALS, CellSlot, StoreCell};
pub use txn::{next_txn_id, txn};
pub use value::{KeyFn, ListLike, SignalValue};
