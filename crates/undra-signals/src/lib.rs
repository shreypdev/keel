#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
//!
//! # Threading
//!
//! Everything is `Send + Sync`, and concurrent writes from several threads are memory-safe and
//! deadlock-free. The Undra runtime additionally serialises all mutation under its core lock
//! (SPEC 5.1), and a signal of a store may only be written on its owning runtime's core: every
//! build asks the embedder's [`set_write_checker`] and refuses any other write with E0065, or
//! returns it as a [`WriteError`] from [`Signal::try_set`] (ADR-035). Each store's change-sets go
//! to its owner ([`ChangeSink::deliver_from`], [`StoreCell::set_owner`]). A transaction belongs to the thread that opened it: it is committed
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
//! # Derived lists
//!
//! [`DerivedList`] (ADR-039) is a filtered, sorted and mapped view of a `Signal<Vec<T>>`, built with
//! [`Signal::derive`] and kept from the list's recorded operations: O(log n) per changed row, at
//! most two keyed-patch ops per source op on the wire, nothing for an op that does not change the
//! view. Its value is always `stable_sort_by_key(filter_map(source))`. Its closures must be pure
//! functions of the row; debug builds panic when one reads or writes reactive state, and a closure
//! that reads its own list panics in every build. A `DerivedList` attached to a store
//! ([`StoreCell::attach_derived`]) is isolated like a computed when a closure panics.
//!
//! # Lazy lists
//!
//! A `Signal<Vec<T>>` reaches the host whole (or as keyed patches); a 100,000-row table should not.
//! [`Lazy`] (ADR-043) is a core-owned list that **never crosses the boundary as a value**: the host
//! is told its length and a *version* (change-set op 0 when it observes it), asks for the window it
//! shows with page calls (a page of 50 rows is encoded on request, microseconds), and is told that
//! something changed by one 12-byte entry (op 2: the new length and version), whatever the change
//! was and however many a transaction made, after which it asks for its window again. Nothing the
//! host does not show is encoded. [`Lazy::new`] and [`Lazy::from_vec`] make an owned list with the
//! recorded API of a `Signal<Vec<T>>`; [`Lazy::over`] makes a read-only view of a
//! [`DerivedList`] that pages through the derived list's index. [`LazySource`] is the type-erased
//! page server the runtime keeps in its object table, and [`StoreCell::attach_lazy`] binds a list
//! to a store slot. A snapshot carries an owned list's items (as the `Vec<T>` of them); a view is
//! derived data and is rebuilt by the store's restore hook.
//!
//! # Panics
//!
//! A panic inside [`txn`], a computed closure, an effect, an encoder or a sink leaves the
//! thread's transaction state consistent (see [`txn`](crate::txn())).
//!
//! A computed that panics while a commit or an observe evaluates it **poisons only itself**
//! (ADR-019 amendment): it is left out of the change-set and held back, every other signal of
//! the store is delivered, the write that triggered the commit succeeds, and the failure is
//! reported once through [`ChangeSink::computed_failed`] and kept as the signal's typed state
//! ([`StoreCell::failed_signals`]). It is evaluated again when its inputs change, and its full
//! value is delivered when that succeeds.
//!
//! Any other panic of a commit (an effect, an encoder, a sink) is re-raised from the outermost
//! write once the commit has finished, so a write can panic if something it triggered did. If
//! encoding or delivering a store's change-set panics, that change-set is abandoned (never
//! half-sent) and the claim is undone: the slots it covered are remembered and sent again, as
//! full values, by the next commit that touches the store (or dropped once the host observes
//! them again), so the host never keeps values the core has moved on from. `observe` is
//! transactional the same way: if an encoder or the delivery panics, no target stays observed
//! because of it and no partial entries are produced.

mod computed;
mod context;
mod deps;
mod derived;
mod effect;
mod error;
mod graph;
mod lazy;
mod oplog;
mod signal;
mod sink;
mod store;
pub mod testing;
mod txn;
mod value;

pub use computed::Computed;
pub use context::{clear_write_checker, set_write_checker};
pub use deps::{Dep, Deps};
pub use derived::{Derive, DerivedList, DerivedStats, Order, Sorted, Unsorted};
pub use effect::Effect;
pub use error::{SignalsError, WriteError};
pub use lazy::{Lazy, LazyHooks, LazySource, page_window};
pub use signal::Signal;
pub use sink::{ChangeSink, clear_sink, set_sink, with_sink};
pub use store::{ALL_SIGNALS, CellSlot, StoreCell};
pub use txn::{next_txn_id, txn};
pub use value::{KeyFn, ListLike, SignalValue};

/// Sorts a list of signal ids in place: Shell's sort, a few instructions, instead of the
/// instantiation of `slice::sort_unstable` for `u32` (about 4 KB of wasm in every core, ADR-052).
/// A store has a few dozen signals and a commit's list is nearly in order, so this is faster than
/// a general sort at that size and fine for thousands.
///
/// ```
/// let mut ids = [5, 1, 4, 1, 3];
/// undra_signals::sort_ids(&mut ids);
/// assert_eq!(ids, [1, 1, 3, 4, 5]);
/// ```
pub fn sort_ids(ids: &mut [u32]) {
    let n = ids.len();
    let mut gap = 1;
    while gap < n / 3 {
        gap = gap * 3 + 1;
    }
    while gap > 0 {
        for i in gap..n {
            let value = ids[i];
            let mut j = i;
            while j >= gap && ids[j - gap] > value {
                ids[j] = ids[j - gap];
                j -= gap;
            }
            ids[j] = value;
        }
        gap /= 3;
    }
}
