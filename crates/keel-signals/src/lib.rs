#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
//!
//! # Threading
//!
//! Everything is `Send + Sync`, and concurrent writes from several threads are memory-safe and
//! deadlock-free. The Keel runtime additionally serialises all mutation under its core lock
//! (SPEC 5.1); that lock, not this crate, is what orders the change-sets of different threads.
//! A transaction belongs to the thread that opened it: it is committed by that thread, and the
//! sink, effects and computed closures it triggers run on that thread.
//!
//! # Panics
//!
//! A panic inside [`txn`], a computed closure, an effect, an encoder or a sink leaves the
//! thread's transaction state consistent (see [`txn`](crate::txn())). The panic is re-raised
//! from the outermost write once the commit has finished, so a write can panic if something it
//! triggered did. If encoding a store's change-set panics, that store's change-set for the
//! transaction is abandoned (never half-sent); the host can resynchronise with `observe`.

mod computed;
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
pub use deps::{Dep, Deps};
pub use effect::Effect;
pub use error::SignalsError;
pub use signal::Signal;
pub use sink::{ChangeSink, clear_sink, set_sink, with_sink};
pub use store::{ALL_SIGNALS, CellSlot, StoreCell};
pub use txn::{next_txn_id, txn};
pub use value::{KeyFn, ListLike, SignalValue};
